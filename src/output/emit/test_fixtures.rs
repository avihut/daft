//! Shared fixtures used across per-format unit tests.
//!
//! Chosen to exercise: nulls, unicode, embedded commas/tabs/newlines, and a
//! realistic row count.

#![allow(dead_code)]

use crate::output::emit::payload::{Cell, EmitPayload, Matrix, Section, Table};

pub fn fixture_tabular() -> EmitPayload {
    EmitPayload::Tabular(
        Table::new(["name", "size", "enabled"])
            .row([Cell::str("alpha"), Cell::int(42i64), Cell::bool(true)])
            .row([
                Cell::str("béta, with comma"),
                Cell::int(0i64),
                Cell::bool(false),
            ])
            .row([Cell::str("gamma"), Cell::null(), Cell::bool(true)]),
    )
}

pub fn fixture_document() -> EmitPayload {
    EmitPayload::Document(serde_json::json!({
        "title": "Release 1.2",
        "date": "2026-04-22",
        "sections": [
            {"heading": "Features", "items": ["foo", "bar"]},
            {"heading": "Fixes", "items": ["baz"]},
        ],
    }))
}

pub fn fixture_matrix() -> EmitPayload {
    let mut m = Matrix::new("path", "worktree", "state");
    m.set("shared/foo.txt", "main", Cell::str("linked"));
    m.set("shared/foo.txt", "feat", Cell::str("materialized"));
    m.set("shared/bar.txt", "main", Cell::str("linked"));
    m.set("shared/bar.txt", "feat", Cell::str("missing"));
    EmitPayload::Matrix(m)
}

pub fn fixture_sectioned() -> EmitPayload {
    EmitPayload::Sectioned(vec![
        Section::new(
            "remotes",
            EmitPayload::Tabular(Table::new(["name", "url", "is_default"]).row([
                Cell::str("origin"),
                Cell::str("git@host:org/repo.git"),
                Cell::bool(true),
            ])),
        ),
        Section::new(
            "worktrees",
            EmitPayload::Tabular(Table::new(["branch", "remote", "path"]).row([
                Cell::str("main"),
                Cell::str("origin"),
                Cell::str("/w/main"),
            ])),
        ),
    ])
}
