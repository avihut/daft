//! Generic dependency graph scheduler.
//!
//! Provides a domain-agnostic DAG that schedules named nodes with
//! dependencies. The scheduler handles ordering, failure cascading,
//! and thread coordination. Domain-specific logic plugs in via the
//! `task_fn` closure passed to [`DagGraph::run_parallel`] or
//! [`DagGraph::run_sequential`].

use super::NodeStatus;
use std::collections::HashMap;
use std::sync::{Condvar, Mutex};

/// A generic dependency graph that can be executed in parallel or sequentially.
///
/// Nodes are identified by index (into [`names`]) and connected by directed
/// edges representing "depends on" relationships.
#[derive(Debug)]
pub struct DagGraph {
    /// Node names.
    pub names: Vec<String>,
    /// Dependencies per node (indices into `names`).
    deps: Vec<Vec<usize>>,
    /// Reverse edges: `dependents[i]` lists nodes that depend on node `i`.
    dependents: Vec<Vec<usize>>,
    /// Initial in-degree per node.
    in_degree: Vec<usize>,
}

impl DagGraph {
    /// Build from `(name, dependency_names)` pairs.
    ///
    /// Returns `Err` if a dependency references a name that does not exist
    /// in the node list, or if a cycle is detected.
    pub fn new(nodes: Vec<(String, Vec<String>)>) -> Result<Self, DagError> {
        let n = nodes.len();
        let name_to_idx: HashMap<&str, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (name.as_str(), i))
            .collect();

        let mut deps: Vec<Vec<usize>> = Vec::with_capacity(n);
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut in_degree: Vec<usize> = vec![0; n];

        for (i, (name, dep_names)) in nodes.iter().enumerate() {
            let mut node_deps = Vec::with_capacity(dep_names.len());
            for dep_name in dep_names {
                let &dep_idx = name_to_idx.get(dep_name.as_str()).ok_or_else(|| {
                    DagError::MissingDependency {
                        node: name.clone(),
                        dependency: dep_name.clone(),
                    }
                })?;
                node_deps.push(dep_idx);
                dependents[dep_idx].push(i);
                in_degree[i] += 1;
            }
            deps.push(node_deps);
        }

        // Cycle detection via Kahn's algorithm: count how many nodes we can
        // topologically sort. If fewer than `n`, there is a cycle.
        let mut topo_in_degree = in_degree.clone();
        let mut queue: Vec<usize> = (0..n).filter(|&i| topo_in_degree[i] == 0).collect();
        let mut visited = 0usize;
        while let Some(idx) = queue.pop() {
            visited += 1;
            for &dep_idx in &dependents[idx] {
                topo_in_degree[dep_idx] -= 1;
                if topo_in_degree[dep_idx] == 0 {
                    queue.push(dep_idx);
                }
            }
        }
        if visited < n {
            return Err(DagError::Cycle);
        }

        let names = nodes.into_iter().map(|(name, _)| name).collect();

        Ok(Self {
            names,
            deps,
            dependents,
            in_degree,
        })
    }

    /// Number of nodes.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the graph has no nodes.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Get the indices of nodes that depend on node `idx`.
    pub fn dependents_of(&self, idx: usize) -> &[usize] {
        &self.dependents[idx]
    }

    /// Get the indices of nodes that node `idx` depends on.
    pub fn dependencies_of(&self, idx: usize) -> &[usize] {
        &self.deps[idx]
    }

    /// Run tasks in parallel using a scoped thread pool with `Condvar`.
    ///
    /// `task_fn` receives `(node_index, node_name)` and returns a
    /// [`NodeStatus`]. The scheduler handles ordering, failure cascading,
    /// and thread coordination.
    pub fn run_parallel<F>(&self, task_fn: F, max_workers: usize) -> Vec<NodeStatus>
    where
        F: Fn(usize, &str) -> NodeStatus + Send + Sync,
    {
        let n = self.names.len();
        if n == 0 {
            return Vec::new();
        }

        let workers = max_workers.max(1);

        struct State {
            ready: Vec<usize>,
            status: Vec<NodeStatus>,
            in_degree: Vec<usize>,
            active: usize,
            done: usize,
        }

        let state = Mutex::new(State {
            ready: (0..n).filter(|&i| self.in_degree[i] == 0).collect(),
            status: vec![NodeStatus::Pending; n],
            in_degree: self.in_degree.clone(),
            active: 0,
            done: 0,
        });
        let cvar = Condvar::new();

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let state = &state;
                let cvar = &cvar;
                let task_fn = &task_fn;

                scope.spawn(move || {
                    loop {
                        let task_idx;
                        {
                            let mut s = state.lock().unwrap();

                            loop {
                                if let Some(idx) = s.ready.pop() {
                                    task_idx = idx;
                                    s.status[task_idx] = NodeStatus::Running;
                                    s.active += 1;
                                    break;
                                }

                                // No ready tasks and no active workers means we are done.
                                if s.active == 0 {
                                    return;
                                }

                                // Wait for something to change.
                                s = cvar.wait(s).unwrap();
                            }
                        }

                        // Execute the task outside the lock.
                        let result_status = task_fn(task_idx, &self.names[task_idx]);

                        // Update DAG state.
                        {
                            let mut s = state.lock().unwrap();
                            s.status[task_idx] = result_status;
                            s.active -= 1;
                            s.done += 1;

                            if result_status == NodeStatus::Succeeded
                                || result_status == NodeStatus::Skipped
                            {
                                for &dep_idx in &self.dependents[task_idx] {
                                    if s.status[dep_idx] == NodeStatus::Pending {
                                        s.in_degree[dep_idx] -= 1;
                                        if s.in_degree[dep_idx] == 0 {
                                            s.ready.push(dep_idx);
                                        }
                                    }
                                }
                            } else if result_status == NodeStatus::Failed {
                                let count =
                                    cascade_dep_failed(&mut s.status, &self.dependents, task_idx);
                                s.done += count;
                            }

                            cvar.notify_all();
                        }
                    }
                });
            }
        });

        let s = state.into_inner().unwrap();
        s.status
    }

    /// Run tasks in topological order sequentially.
    ///
    /// `task_fn` receives `(node_index, node_name)` and returns a
    /// [`NodeStatus`].
    pub fn run_sequential<F>(&self, task_fn: F) -> Vec<NodeStatus>
    where
        F: Fn(usize, &str) -> NodeStatus,
    {
        let n = self.names.len();
        if n == 0 {
            return Vec::new();
        }

        let mut status = vec![NodeStatus::Pending; n];
        let mut in_degree = self.in_degree.clone();
        let mut ready: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();

        while let Some(idx) = ready.pop() {
            // Skip nodes that were dep-failed by a prior cascade.
            if status[idx] == NodeStatus::DepFailed {
                continue;
            }

            let result_status = task_fn(idx, &self.names[idx]);
            status[idx] = result_status;

            if result_status == NodeStatus::Succeeded || result_status == NodeStatus::Skipped {
                for &dep_idx in &self.dependents[idx] {
                    if status[dep_idx] == NodeStatus::Pending {
                        in_degree[dep_idx] -= 1;
                        if in_degree[dep_idx] == 0 {
                            ready.push(dep_idx);
                        }
                    }
                }
            } else if result_status == NodeStatus::Failed {
                cascade_dep_failed(&mut status, &self.dependents, idx);
                // Enqueue dep-failed nodes so we can pop-and-skip them,
                // ensuring the loop terminates even when dependents
                // were waiting on this node.
                for &dep_idx in &self.dependents[idx] {
                    if status[dep_idx] == NodeStatus::DepFailed {
                        // Recursively enqueue all transitive dep-failed nodes.
                        let mut stack = vec![dep_idx];
                        while let Some(df_idx) = stack.pop() {
                            // Already marked DepFailed; just make sure it
                            // ends up in the ready queue so the main loop
                            // can skip over it (harmless if duplicated).
                            ready.push(df_idx);
                            for &child in &self.dependents[df_idx] {
                                if status[child] == NodeStatus::DepFailed {
                                    stack.push(child);
                                }
                            }
                        }
                    }
                }
            }
        }

        status
    }
}

/// Cascade `DepFailed` to all transitive dependents of a failed node.
///
/// Uses a stack-based DFS traversal. Only nodes that are still `Pending`
/// are marked as `DepFailed`.
///
/// Returns the count of newly dep-failed nodes.
pub fn cascade_dep_failed(
    statuses: &mut [NodeStatus],
    dependents: &[Vec<usize>],
    failed_idx: usize,
) -> usize {
    let mut count = 0;
    let mut stack = vec![failed_idx];
    while let Some(idx) = stack.pop() {
        for &dep_idx in &dependents[idx] {
            if statuses[dep_idx] == NodeStatus::Pending {
                statuses[dep_idx] = NodeStatus::DepFailed;
                count += 1;
                stack.push(dep_idx);
            }
        }
    }
    count
}

/// Errors that can occur when constructing a [`DagGraph`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DagError {
    /// A node references a dependency that does not exist.
    MissingDependency {
        /// The node that declared the dependency.
        node: String,
        /// The dependency name that was not found.
        dependency: String,
    },
    /// The graph contains a cycle.
    Cycle,
}

impl std::fmt::Display for DagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingDependency { node, dependency } => {
                write!(
                    f,
                    "node '{node}' depends on '{dependency}', which does not exist"
                )
            }
            Self::Cycle => write!(f, "dependency graph contains a cycle"),
        }
    }
}

impl std::error::Error for DagError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // ── Graph construction ──────────────────────────────────────────────

    #[test]
    fn empty_graph() {
        let dag = DagGraph::new(vec![]).unwrap();
        assert!(dag.is_empty());
        assert_eq!(dag.len(), 0);
    }

    #[test]
    fn single_node() {
        let dag = DagGraph::new(vec![("A".into(), vec![])]).unwrap();
        assert_eq!(dag.len(), 1);
        assert!(!dag.is_empty());
        assert!(dag.dependencies_of(0).is_empty());
        assert!(dag.dependents_of(0).is_empty());
    }

    #[test]
    fn linear_chain() {
        // A -> B -> C
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["B".into()]),
        ])
        .unwrap();

        assert_eq!(dag.len(), 3);
        assert!(dag.dependencies_of(0).is_empty());
        assert_eq!(dag.dependencies_of(1), &[0]);
        assert_eq!(dag.dependencies_of(2), &[1]);
        assert_eq!(dag.dependents_of(0), &[1]);
        assert_eq!(dag.dependents_of(1), &[2]);
        assert!(dag.dependents_of(2).is_empty());
    }

    #[test]
    fn diamond_graph() {
        // A -> B, A -> C, B -> D, C -> D
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["A".into()]),
            ("D".into(), vec!["B".into(), "C".into()]),
        ])
        .unwrap();

        assert_eq!(dag.len(), 4);
        assert_eq!(dag.in_degree[0], 0);
        assert_eq!(dag.in_degree[1], 1);
        assert_eq!(dag.in_degree[2], 1);
        assert_eq!(dag.in_degree[3], 2);
    }

    #[test]
    fn missing_dependency_error() {
        let result = DagGraph::new(vec![("A".into(), vec!["B".into()])]);
        assert!(result.is_err());
        match result.unwrap_err() {
            DagError::MissingDependency { node, dependency } => {
                assert_eq!(node, "A");
                assert_eq!(dependency, "B");
            }
            other => panic!("expected MissingDependency, got {other:?}"),
        }
    }

    #[test]
    fn cycle_detection() {
        // A -> B -> C -> A
        let result = DagGraph::new(vec![
            ("A".into(), vec!["C".into()]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["B".into()]),
        ]);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), DagError::Cycle);
    }

    #[test]
    fn self_cycle_detection() {
        let result = DagGraph::new(vec![("A".into(), vec!["A".into()])]);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), DagError::Cycle);
    }

    // ── DagError display ────────────────────────────────────────────────

    #[test]
    fn dag_error_display_missing() {
        let err = DagError::MissingDependency {
            node: "X".into(),
            dependency: "Y".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("X"));
        assert!(msg.contains("Y"));
        assert!(msg.contains("does not exist"));
    }

    #[test]
    fn dag_error_display_cycle() {
        let err = DagError::Cycle;
        let msg = err.to_string();
        assert!(msg.contains("cycle"));
    }

    #[test]
    fn dag_error_is_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(DagError::Cycle);
        let _ = err.to_string();
    }

    // ── Sequential execution ────────────────────────────────────────────

    #[test]
    fn sequential_empty_graph() {
        let dag = DagGraph::new(vec![]).unwrap();
        let results = dag.run_sequential(|_, _| NodeStatus::Succeeded);
        assert!(results.is_empty());
    }

    #[test]
    fn sequential_single_node_succeeds() {
        let dag = DagGraph::new(vec![("A".into(), vec![])]).unwrap();
        let results = dag.run_sequential(|_, _| NodeStatus::Succeeded);
        assert_eq!(results, vec![NodeStatus::Succeeded]);
    }

    #[test]
    fn sequential_all_succeed() {
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["B".into()]),
        ])
        .unwrap();

        let results = dag.run_sequential(|_, _| NodeStatus::Succeeded);
        assert_eq!(
            results,
            vec![
                NodeStatus::Succeeded,
                NodeStatus::Succeeded,
                NodeStatus::Succeeded
            ]
        );
    }

    #[test]
    fn sequential_topological_order() {
        // A -> B -> C
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["B".into()]),
        ])
        .unwrap();

        let order = Mutex::new(Vec::new());
        dag.run_sequential(|idx, name| {
            order.lock().unwrap().push((idx, name.to_string()));
            NodeStatus::Succeeded
        });

        let order = order.into_inner().unwrap();
        // A must come before B, B must come before C.
        let pos_a = order.iter().position(|(_, n)| n == "A").unwrap();
        let pos_b = order.iter().position(|(_, n)| n == "B").unwrap();
        let pos_c = order.iter().position(|(_, n)| n == "C").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn sequential_failure_cascades() {
        // A -> B -> C
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["B".into()]),
        ])
        .unwrap();

        let results = dag.run_sequential(|_, name| {
            if name == "A" {
                NodeStatus::Failed
            } else {
                NodeStatus::Succeeded
            }
        });

        assert_eq!(results[0], NodeStatus::Failed);
        assert_eq!(results[1], NodeStatus::DepFailed);
        assert_eq!(results[2], NodeStatus::DepFailed);
    }

    #[test]
    fn sequential_middle_failure() {
        // A -> B -> C
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["B".into()]),
        ])
        .unwrap();

        let results = dag.run_sequential(|_, name| {
            if name == "B" {
                NodeStatus::Failed
            } else {
                NodeStatus::Succeeded
            }
        });

        assert_eq!(results[0], NodeStatus::Succeeded);
        assert_eq!(results[1], NodeStatus::Failed);
        assert_eq!(results[2], NodeStatus::DepFailed);
    }

    #[test]
    fn sequential_skipped_unlocks_dependents() {
        // A -> B
        let dag =
            DagGraph::new(vec![("A".into(), vec![]), ("B".into(), vec!["A".into()])]).unwrap();

        let results = dag.run_sequential(|_, name| {
            if name == "A" {
                NodeStatus::Skipped
            } else {
                NodeStatus::Succeeded
            }
        });

        assert_eq!(results[0], NodeStatus::Skipped);
        assert_eq!(results[1], NodeStatus::Succeeded);
    }

    #[test]
    fn sequential_diamond_all_succeed() {
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["A".into()]),
            ("D".into(), vec!["B".into(), "C".into()]),
        ])
        .unwrap();

        let results = dag.run_sequential(|_, _| NodeStatus::Succeeded);
        assert!(results.iter().all(|s| *s == NodeStatus::Succeeded));
    }

    #[test]
    fn sequential_diamond_one_branch_fails() {
        // A -> B, A -> C, B -> D, C -> D
        // B fails => D gets DepFailed, C still succeeds
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["A".into()]),
            ("D".into(), vec!["B".into(), "C".into()]),
        ])
        .unwrap();

        let results = dag.run_sequential(|_, name| {
            if name == "B" {
                NodeStatus::Failed
            } else {
                NodeStatus::Succeeded
            }
        });

        assert_eq!(results[0], NodeStatus::Succeeded); // A
        assert_eq!(results[1], NodeStatus::Failed); // B
        assert_eq!(results[2], NodeStatus::Succeeded); // C
        assert_eq!(results[3], NodeStatus::DepFailed); // D (depends on B which failed)
    }

    #[test]
    fn sequential_independent_nodes() {
        // A, B, C — no dependencies
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec![]),
            ("C".into(), vec![]),
        ])
        .unwrap();

        let results = dag.run_sequential(|_, _| NodeStatus::Succeeded);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|s| *s == NodeStatus::Succeeded));
    }

    // ── Parallel execution ──────────────────────────────────────────────

    #[test]
    fn parallel_empty_graph() {
        let dag = DagGraph::new(vec![]).unwrap();
        let results = dag.run_parallel(|_, _| NodeStatus::Succeeded, 4);
        assert!(results.is_empty());
    }

    #[test]
    fn parallel_single_node() {
        let dag = DagGraph::new(vec![("A".into(), vec![])]).unwrap();
        let results = dag.run_parallel(|_, _| NodeStatus::Succeeded, 4);
        assert_eq!(results, vec![NodeStatus::Succeeded]);
    }

    #[test]
    fn parallel_all_succeed() {
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["A".into()]),
            ("D".into(), vec!["B".into(), "C".into()]),
        ])
        .unwrap();

        let results = dag.run_parallel(|_, _| NodeStatus::Succeeded, 4);
        assert!(results.iter().all(|s| *s == NodeStatus::Succeeded));
    }

    #[test]
    fn parallel_dependencies_respected() {
        // A -> B -> C
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["B".into()]),
        ])
        .unwrap();

        let order = Arc::new(Mutex::new(Vec::new()));
        let order_clone = Arc::clone(&order);

        dag.run_parallel(
            move |idx, _name| {
                order_clone.lock().unwrap().push(idx);
                NodeStatus::Succeeded
            },
            4,
        );

        let order = order.lock().unwrap();
        let pos_a = order.iter().position(|&i| i == 0).unwrap();
        let pos_b = order.iter().position(|&i| i == 1).unwrap();
        let pos_c = order.iter().position(|&i| i == 2).unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn parallel_failure_cascades() {
        // A -> B -> C
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["B".into()]),
        ])
        .unwrap();

        let results = dag.run_parallel(
            |_, name| {
                if name == "A" {
                    NodeStatus::Failed
                } else {
                    NodeStatus::Succeeded
                }
            },
            4,
        );

        assert_eq!(results[0], NodeStatus::Failed);
        assert_eq!(results[1], NodeStatus::DepFailed);
        assert_eq!(results[2], NodeStatus::DepFailed);
    }

    #[test]
    fn parallel_diamond_one_branch_fails() {
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["A".into()]),
            ("D".into(), vec!["B".into(), "C".into()]),
        ])
        .unwrap();

        let results = dag.run_parallel(
            |_, name| {
                if name == "B" {
                    NodeStatus::Failed
                } else {
                    NodeStatus::Succeeded
                }
            },
            4,
        );

        assert_eq!(results[0], NodeStatus::Succeeded); // A
        assert_eq!(results[1], NodeStatus::Failed); // B
        assert_eq!(results[2], NodeStatus::Succeeded); // C
        assert_eq!(results[3], NodeStatus::DepFailed); // D
    }

    #[test]
    fn parallel_skipped_unlocks_dependents() {
        let dag =
            DagGraph::new(vec![("A".into(), vec![]), ("B".into(), vec!["A".into()])]).unwrap();

        let results = dag.run_parallel(
            |_, name| {
                if name == "A" {
                    NodeStatus::Skipped
                } else {
                    NodeStatus::Succeeded
                }
            },
            4,
        );

        assert_eq!(results[0], NodeStatus::Skipped);
        assert_eq!(results[1], NodeStatus::Succeeded);
    }

    #[test]
    fn parallel_single_worker() {
        // Even with 1 worker, all tasks should complete.
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["B".into()]),
        ])
        .unwrap();

        let results = dag.run_parallel(|_, _| NodeStatus::Succeeded, 1);
        assert!(results.iter().all(|s| *s == NodeStatus::Succeeded));
    }

    #[test]
    fn parallel_many_independent_nodes() {
        // 20 independent nodes — test parallelism under load.
        let nodes: Vec<(String, Vec<String>)> =
            (0..20).map(|i| (format!("N{i}"), vec![])).collect();
        let dag = DagGraph::new(nodes).unwrap();

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);

        let results = dag.run_parallel(
            move |_, _| {
                counter_clone.fetch_add(1, Ordering::Relaxed);
                NodeStatus::Succeeded
            },
            4,
        );

        assert_eq!(results.len(), 20);
        assert!(results.iter().all(|s| *s == NodeStatus::Succeeded));
        assert_eq!(counter.load(Ordering::Relaxed), 20);
    }

    #[test]
    fn parallel_zero_workers_treated_as_one() {
        let dag = DagGraph::new(vec![("A".into(), vec![])]).unwrap();
        let results = dag.run_parallel(|_, _| NodeStatus::Succeeded, 0);
        assert_eq!(results, vec![NodeStatus::Succeeded]);
    }

    // ── cascade_dep_failed standalone ───────────────────────────────────

    #[test]
    fn cascade_dep_failed_no_dependents() {
        let mut statuses = vec![NodeStatus::Failed];
        let dependents: Vec<Vec<usize>> = vec![vec![]];
        let count = cascade_dep_failed(&mut statuses, &dependents, 0);
        assert_eq!(count, 0);
        assert_eq!(statuses[0], NodeStatus::Failed);
    }

    #[test]
    fn cascade_dep_failed_one_level() {
        // 0 -> 1, 0 -> 2
        let mut statuses = vec![NodeStatus::Failed, NodeStatus::Pending, NodeStatus::Pending];
        let dependents = vec![vec![1, 2], vec![], vec![]];
        let count = cascade_dep_failed(&mut statuses, &dependents, 0);
        assert_eq!(count, 2);
        assert_eq!(statuses[1], NodeStatus::DepFailed);
        assert_eq!(statuses[2], NodeStatus::DepFailed);
    }

    #[test]
    fn cascade_dep_failed_transitive() {
        // 0 -> 1 -> 2 -> 3
        let mut statuses = vec![
            NodeStatus::Failed,
            NodeStatus::Pending,
            NodeStatus::Pending,
            NodeStatus::Pending,
        ];
        let dependents = vec![vec![1], vec![2], vec![3], vec![]];
        let count = cascade_dep_failed(&mut statuses, &dependents, 0);
        assert_eq!(count, 3);
        assert_eq!(statuses[1], NodeStatus::DepFailed);
        assert_eq!(statuses[2], NodeStatus::DepFailed);
        assert_eq!(statuses[3], NodeStatus::DepFailed);
    }

    #[test]
    fn cascade_dep_failed_skips_non_pending() {
        // 0 -> 1, 0 -> 2; node 2 already succeeded
        let mut statuses = vec![
            NodeStatus::Failed,
            NodeStatus::Pending,
            NodeStatus::Succeeded,
        ];
        let dependents = vec![vec![1, 2], vec![], vec![]];
        let count = cascade_dep_failed(&mut statuses, &dependents, 0);
        assert_eq!(count, 1);
        assert_eq!(statuses[1], NodeStatus::DepFailed);
        assert_eq!(statuses[2], NodeStatus::Succeeded); // unchanged
    }

    #[test]
    fn cascade_dep_failed_diamond() {
        // 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        let mut statuses = vec![
            NodeStatus::Failed,
            NodeStatus::Pending,
            NodeStatus::Pending,
            NodeStatus::Pending,
        ];
        let dependents = vec![vec![1, 2], vec![3], vec![3], vec![]];
        let count = cascade_dep_failed(&mut statuses, &dependents, 0);
        // All 3 pending nodes get DepFailed.
        assert_eq!(count, 3);
        assert_eq!(statuses[1], NodeStatus::DepFailed);
        assert_eq!(statuses[2], NodeStatus::DepFailed);
        assert_eq!(statuses[3], NodeStatus::DepFailed);
    }

    // ── Mixed results ───────────────────────────────────────────────────

    #[test]
    fn mixed_results_parallel() {
        // A (succeed) -> B (fail) -> D (dep-failed)
        // A (succeed) -> C (succeed)
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["A".into()]),
            ("D".into(), vec!["B".into()]),
        ])
        .unwrap();

        let results = dag.run_parallel(
            |_, name| match name {
                "B" => NodeStatus::Failed,
                _ => NodeStatus::Succeeded,
            },
            2,
        );

        assert_eq!(results[0], NodeStatus::Succeeded);
        assert_eq!(results[1], NodeStatus::Failed);
        assert_eq!(results[2], NodeStatus::Succeeded);
        assert_eq!(results[3], NodeStatus::DepFailed);
    }

    #[test]
    fn mixed_results_sequential() {
        let dag = DagGraph::new(vec![
            ("A".into(), vec![]),
            ("B".into(), vec!["A".into()]),
            ("C".into(), vec!["A".into()]),
            ("D".into(), vec!["B".into()]),
        ])
        .unwrap();

        let results = dag.run_sequential(|_, name| match name {
            "B" => NodeStatus::Failed,
            _ => NodeStatus::Succeeded,
        });

        assert_eq!(results[0], NodeStatus::Succeeded);
        assert_eq!(results[1], NodeStatus::Failed);
        assert_eq!(results[2], NodeStatus::Succeeded);
        assert_eq!(results[3], NodeStatus::DepFailed);
    }

    // ── Wide fan-out ────────────────────────────────────────────────────

    #[test]
    fn wide_fan_out_parallel() {
        // Root -> 10 children
        let mut nodes: Vec<(String, Vec<String>)> = vec![("root".into(), vec![])];
        for i in 0..10 {
            nodes.push((format!("child-{i}"), vec!["root".into()]));
        }
        let dag = DagGraph::new(nodes).unwrap();
        let results = dag.run_parallel(|_, _| NodeStatus::Succeeded, 4);
        assert_eq!(results.len(), 11);
        assert!(results.iter().all(|s| *s == NodeStatus::Succeeded));
    }

    #[test]
    fn wide_fan_out_root_fails() {
        let mut nodes: Vec<(String, Vec<String>)> = vec![("root".into(), vec![])];
        for i in 0..10 {
            nodes.push((format!("child-{i}"), vec!["root".into()]));
        }
        let dag = DagGraph::new(nodes).unwrap();
        let results = dag.run_parallel(
            |_, name| {
                if name == "root" {
                    NodeStatus::Failed
                } else {
                    NodeStatus::Succeeded
                }
            },
            4,
        );
        assert_eq!(results[0], NodeStatus::Failed);
        for s in &results[1..] {
            assert_eq!(*s, NodeStatus::DepFailed);
        }
    }
}
