//! Detect development-schema drift without a migration ladder or whole-library reset.
use std::collections::BTreeMap;

use rusqlite::Connection;

use super::{AppError, Result, SCHEMA_SQL, SCHEMA_VERSION};

// These rows describe one coherent sync history. Clearing only one would leave
// old cursors/completion claims or issued work attached to a newly empty table.
// Configuration, accepted edits, account selection and media are not history.
const SYNC_HISTORY_TABLES: &[&str] = &[
    "sync_conflicts",
    "sync_tasks",
    "sync_task_resources",
    "sync_publications",
    "sync_native_resources",
    "sync_native_progress",
    "sync_entries",
    "sync_record_fields",
    "sync_records",
    "sync_bootstrap_types",
];

#[derive(Debug, PartialEq, Eq)]
struct Column {
    name: String,
    kind: String,
    not_null: bool,
    default: Option<String>,
    primary_key: i64,
    hidden: i64,
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<Column>> {
    let mut stmt = conn.prepare(
        "SELECT name,type,\"notnull\",dflt_value,pk,hidden FROM pragma_table_xinfo(?1) ORDER BY name",
    )?;
    let rows = stmt.query_map([table], |r| {
        Ok(Column {
            name: r.get(0)?,
            kind: r.get::<_, String>(1)?.trim().to_ascii_uppercase(),
            not_null: r.get(2)?,
            default: r.get(3)?,
            primary_key: r.get(4)?,
            hidden: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn table_names(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// A table reset must not implicitly cascade into user data in other tables.
// History rows are deliberately discarded below; any other dependent data
// makes automatic recovery unsafe, so report it before changing anything.
fn protect_dependents(conn: &Connection, target: &str, tables: &[String]) -> Result<()> {
    for table in tables {
        if table == target || SYNC_HISTORY_TABLES.contains(&table.as_str()) {
            continue;
        }
        let mut stmt =
            conn.prepare("SELECT \"from\" FROM pragma_foreign_key_list(?1) WHERE \"table\"=?2")?;
        let references = stmt.query_map([table.as_str(), target], |r| r.get::<_, String>(0))?;
        for column in references {
            let column = column?;
            let populated: bool = conn.query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM {} WHERE {} IS NOT NULL)",
                    quote(table),
                    quote(&column)
                ),
                [],
                |r| r.get(0),
            )?;
            if populated {
                return Err(AppError::Backend(format!(
                    "database schema mismatch in {target}: cannot rebuild one table without losing dependent data in {table}.{column}"
                )));
            }
        }
    }
    Ok(())
}

/// Runs before the pool, actor, scanner and sync service exist. SQLite's schema
/// and row changes share one transaction, so any failure restores the old data.
pub(super) fn initialize(conn: &mut Connection) -> Result<()> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(SCHEMA_SQL)?;
    let expected: BTreeMap<_, _> = table_names(&expected)?
        .into_iter()
        .map(|name| columns(&expected, &name).map(|columns| (name, columns)))
        .collect::<Result<_>>()?;

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let tables = table_names(&tx)?;
    let legacy_state = tables
        .iter()
        .map(|table| {
            let cols = columns(&tx, table)?;
            Ok(cols.iter().any(|c| {
                crate::core::sync::store::state::legacy_columns(table).contains(&c.name.as_str())
            }))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .any(|legacy| legacy);
    if legacy_state {
        let definitions = SCHEMA_SQL
            .split("-- Typed native record fragments")
            .nth(1)
            .and_then(|s| s.split_once('\n').map(|(_, sql)| sql))
            .ok_or_else(|| AppError::Backend("missing typed sync schema".into()))?;
        tx.execute_batch(definitions)?;
    }
    let mut mismatches = Vec::new();
    for table in &tables {
        let Some(wanted) = expected.get(table) else {
            continue;
        };
        let mut actual = columns(&tx, table)?;
        // Add current-schema fields without a version ladder, table rebuild,
        // or loss of pending deletion records and existing library metadata.
        let additions = wanted
            .iter()
            .filter(|c| !actual.iter().any(|a| a.name == c.name))
            .collect::<Vec<_>>();
        let compatible = actual
            .iter()
            .filter(|a| {
                !crate::core::sync::store::state::legacy_columns(table).contains(&a.name.as_str())
            })
            .all(|a| wanted.iter().any(|w| w == a));
        if compatible
            && !additions.is_empty()
            && additions
                .iter()
                .all(|c| c.primary_key == 0 && (!c.not_null || c.default.is_some()))
        {
            for column in additions {
                let declaration = format!(
                    "{} {}{}{}{}",
                    quote(&column.name),
                    column.kind,
                    if column.not_null { " NOT NULL" } else { "" },
                    column
                        .default
                        .as_ref()
                        .map(|v| format!(" DEFAULT {v}"))
                        .unwrap_or_default(),
                    if table == "sync_connections" && column.name == "native_account_id" {
                        " REFERENCES sync_accounts(id) ON DELETE SET NULL"
                    } else {
                        ""
                    }
                );
                tx.execute(
                    &format!("ALTER TABLE {} ADD COLUMN {declaration}", quote(table)),
                    [],
                )?;
            }
        }
        crate::core::sync::store::state::migrate_legacy_table(&tx, table)?;
        actual = columns(&tx, table)?;
        if &actual != wanted {
            let missing: Vec<_> = wanted
                .iter()
                .filter(|c| !actual.iter().any(|a| a.name == c.name))
                .map(|c| c.name.as_str())
                .collect();
            let unexpected: Vec<_> = actual
                .iter()
                .filter(|c| !wanted.iter().any(|w| w.name == c.name))
                .map(|c| c.name.as_str())
                .collect();
            let changed: Vec<_> = wanted
                .iter()
                .filter(|c| actual.iter().any(|a| a.name == c.name && a != *c))
                .map(|c| c.name.as_str())
                .collect();
            tracing::warn!(
                target: crate::core::log_targets::STORAGE,
                table, ?missing, ?unexpected, ?changed,
                "database column layout does not match the embedded schema"
            );
            mismatches.push(table.as_str());
        }
    }

    if mismatches.len() > 1 {
        return Err(AppError::Backend(format!(
            "database schema mismatch in multiple tables: {}; automatic recovery may rebuild at most one table",
            mismatches.join(", ")
        )));
    }
    if let Some(table) = mismatches.first() {
        protect_dependents(&tx, table, &tables)?;
        for history in SYNC_HISTORY_TABLES {
            if tables.iter().any(|name| name == history) {
                tx.execute(&format!("DELETE FROM {}", quote(history)), [])?;
            }
        }
        // Exactly one DROP. Foreign keys are checked after the same table has
        // been recreated, rather than allowing DROP to cascade into other data.
        tx.execute(&format!("DROP TABLE {}", quote(table)), [])?;
    }

    // Missing tables/indexes can be created normally; they are not mismatches.
    tx.execute_batch(SCHEMA_SQL)?;
    crate::core::sync::store::records::migrate_legacy(&tx)?;
    // Replaced by canonical photo metadata and per-photo incomplete fragments.
    // Bootstrap once into the new representation; preserve every dirty/version,
    // confirmed identity, issued operation and owned publication. Old metadata
    // may have advanced its cursor without a canonical media row.
    if tables.iter().any(|name| {
        matches!(
            name.as_str(),
            "sync_native_inbox" | "sync_native_records" | "sync_inventory"
        )
    }) {
        tx.execute("UPDATE sync_native_progress SET snapshot_token=NULL,bootstrap_started=0,ingest_token=NULL,target_token=NULL,applied_token=NULL,discovery_token=NULL,discovery_complete=0",[])?;
        tx.execute("DELETE FROM sync_records WHERE owner_kind='pending'", [])?;
        tx.execute(
            "UPDATE sync_entries SET native_has_pending_master=0,native_has_pending_asset=0",
            [],
        )?;
    }
    tx.execute_batch(
        "DROP TABLE IF EXISTS sync_native_inbox; DROP TABLE IF EXISTS sync_native_records; DROP TABLE IF EXISTS sync_inventory;",
    )?;
    if !mismatches.is_empty() {
        tx.execute(
            "UPDATE sync_jobs SET native_cycle_id=NULL,last_started_at=NULL,last_completed_at=NULL,last_error=NULL",
            [],
        )?;
        if tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
            return Err(AppError::Backend(
                "database table recovery would leave dangling foreign keys; all changes were rolled back".into(),
            ));
        }
    }
    crate::core::sync::store::state::migrate_account_selection(&tx)?;
    crate::core::sync::store::state::migrate_reset_intent(&tx)?;
    if legacy_state && tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(AppError::Backend("relational synchronization conversion left dangling references; all changes were rolled back".into()));
    }

    // Availability fields may have been added with local-only defaults while
    // confirmed identities survived. Recover them on every open, including
    // databases already opened once by a build that left those defaults behind.
    crate::core::sync::store::accounts::refresh_media_locations_in(&tx)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    if let Some(table) = mismatches.first() {
        tracing::warn!(
            target: crate::core::log_targets::STORAGE,
            table,
            "rebuilt one incompatible table; cleared sync history for a full synchronization"
        );
    }
    Ok(())
}
