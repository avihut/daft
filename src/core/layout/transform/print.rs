//! Human-readable plan formatting for dry-run output and verbose mode.

use super::execute::describe_op;
use super::plan::TransformPlan;
use crate::output::Output;

/// Print the full transform plan to the output.
pub fn print_plan(plan: &TransformPlan, output: &mut dyn Output) {
    output.step(&format!("Transform plan ({} operations):", plan.ops.len()));

    for (i, op) in plan.ops.iter().enumerate() {
        output.step(&format!("  {}. {}", i + 1, describe_op(op)));
    }

    if !plan.skipped.is_empty() {
        output.step(&format!(
            "\nSkipped ({} non-conforming):",
            plan.skipped.len()
        ));
        for cw in &plan.skipped {
            output.step(&format!(
                "  '{}': {} (use --include to relocate)",
                cw.branch,
                cw.current_path.display()
            ));
        }
    }
}
