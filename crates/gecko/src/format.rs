//! A result's `data` for a reader (§6, UX U6): rows as a table cut to the terminal,
//! a `columns` + `rows` grid as one table, other fields as `key: value`. `--json`
//! keeps it all; this is only the plain view.

use serde_json::{Map, Value};

use crate::output::escape_controls;

/// The longest cell of a table, in code points.
const CELL_MAX: usize = 40;
/// The longest value of a `key: value` line, in code points.
const VALUE_MAX: usize = 120;
/// How many first rows give a table its columns.
const COLUMN_ROWS: usize = 20;

/// Duration fields by name; `startMs` and `endMs` are epochs and stay numbers.
const DURATION_FIELDS: [&str; 7] = [
    "durationMs",
    "runningMs",
    "ageMs",
    "snapshotAgeMs",
    "dataAgeMs",
    "elapsedMs",
    "uptimeMs",
];

fn width(text: &str) -> usize {
    text.chars().count()
}

/// Cuts text to `max` code points, the last one an ellipsis.
fn cut(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A value on one line: strings as they are, the rest as JSON, white space folded.
fn flat(value: &Value) -> String {
    let text = match value {
        Value::Null => return String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    escape_controls(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 454558943 → "5 d 6 h": the two largest units from the first one.
pub fn human_duration(ms: f64) -> String {
    if ms < 1000.0 {
        return format!("{} ms", ms.round());
    }
    const UNITS: [(&str, u64); 4] = [
        ("d", 86_400_000),
        ("h", 3_600_000),
        ("min", 60_000),
        ("s", 1000),
    ];
    // Under 2^53 a duration is exact; anything above is no real duration anyway.
    let mut rest = ms.floor().min(9e15) as u64;
    let first = UNITS
        .iter()
        .position(|(_, size)| rest >= *size)
        .unwrap_or(UNITS.len() - 1);
    let mut parts = Vec::new();
    for (name, size) in UNITS.iter().skip(first).take(2) {
        let count = rest / size;
        rest -= count * size;
        if count > 0 {
            parts.push(format!("{count} {name}"));
        }
    }
    parts.join(" ")
}

/// A field's value for a reader: a known duration in words, else as is.
fn shown(key: &str, value: &Value) -> Value {
    match value.as_f64() {
        Some(ms) if DURATION_FIELDS.contains(&key) && ms.is_finite() && ms >= 0.0 => {
            Value::String(human_duration(ms))
        }
        _ => value.clone(),
    }
}

fn is_rows(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|rows| !rows.is_empty() && rows.iter().all(Value::is_object))
}

/// `null`, `""` and a missing field are empty; `0` and `false` are not.
fn is_empty(value: &Value) -> bool {
    value.is_null() || value.as_str() == Some("")
}

/// A table cell: an array is `[n]`, or its members' `title`, `name` or `value`
/// joined when every member has one; an object is `{…}`.
fn cell(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            for key in ["title", "name", "value"] {
                let named: Option<Vec<String>> = items
                    .iter()
                    .map(|item| match item.get(key) {
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(Value::Number(n)) => Some(n.to_string()),
                        _ => None,
                    })
                    .collect();
                if let Some(named) = named
                    && !named.is_empty()
                {
                    return flat(&Value::String(named.join(", ")));
                }
            }
            format!("[{}]", items.len())
        }
        Value::Object(_) => "{…}".to_owned(),
        other => flat(other),
    }
}

fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `id`, `workId`, `…Id` or a column of UUIDs: never cut.
fn is_id_column(name: &str, values: &[Value]) -> bool {
    if name == "id" || name.ends_with("Id") {
        return true;
    }
    let present: Vec<&str> = values
        .iter()
        .filter_map(Value::as_str)
        .filter(|v| !v.is_empty())
        .collect();
    !present.is_empty() && present.iter().all(|v| is_uuid(v))
}

/// `data` as text for a terminal `columns` wide (`None`: not a terminal).
pub fn format_data(data: &Value, columns: Option<usize>) -> String {
    match data {
        _ if is_rows(data) => table(data.as_array().map_or(&[][..], Vec::as_slice), columns),
        Value::Array(items) if items.is_empty() => "(empty)".to_owned(),
        Value::Array(items) => items
            .iter()
            .map(|item| format!("- {}", cut(&flat(item), VALUE_MAX)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(object) => fields(object, columns),
        Value::String(text) => escape_controls(text),
        other => flat(other),
    }
}

/// `{ columns: string[], rows: unknown[][] }`: a table sent as a grid.
fn grid_of(object: &Map<String, Value>) -> Option<(Vec<String>, Vec<Vec<Value>>)> {
    let columns = object
        .get("columns")?
        .as_array()?
        .iter()
        .map(|c| c.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    let rows = object
        .get("rows")?
        .as_array()?
        .iter()
        .map(|r| r.as_array().cloned())
        .collect::<Option<Vec<_>>>()?;
    Some((columns, rows))
}

fn fields(object: &Map<String, Value>, columns: Option<usize>) -> String {
    let mut lines = Vec::new();
    let mut tables = Vec::new();
    let gridded = grid_of(object);
    if let Some((heads, rows)) = &gridded {
        tables.push(grid(heads, rows, columns));
    }
    for (key, value) in object {
        if gridded.is_some() && (key == "columns" || key == "rows") {
            continue;
        }
        let name = escape_controls(key);
        if is_rows(value) {
            let rows = value.as_array().map_or(&[][..], Vec::as_slice);
            tables.push(format!("{name}:\n{}", table(rows, columns)));
        } else if value.as_array().is_some_and(Vec::is_empty) {
            lines.push(format!("{name}: (empty)"));
        } else {
            lines.push(format!(
                "{name}: {}",
                cut(&flat(&shown(key, value)), VALUE_MAX)
            ));
        }
    }
    lines.extend(tables);
    lines.join("\n")
}

fn table(rows: &[Value], columns: Option<usize>) -> String {
    let mut heads: Vec<String> = Vec::new();
    for row in rows.iter().take(COLUMN_ROWS) {
        for key in row.as_object().into_iter().flat_map(Map::keys) {
            if !heads.contains(key) {
                heads.push(key.clone());
            }
        }
    }
    let cells: Vec<Vec<Value>> = rows
        .iter()
        .map(|row| {
            heads
                .iter()
                .map(|h| row.get(h).cloned().unwrap_or(Value::Null))
                .collect()
        })
        .collect();
    grid(&heads, &cells, columns)
}

/// Rows under a header: columns empty in every row are hidden and named; id columns
/// are never cut, and when they take more than half of the terminal the rows go
/// vertical (`field  value` blocks); other cells are cut at `CELL_MAX`; the columns
/// that do not fit are named.
fn grid(all_columns: &[String], all_cells: &[Vec<Value>], columns_max: Option<usize>) -> String {
    let at = |row: &Vec<Value>, i: usize| row.get(i).cloned().unwrap_or(Value::Null);
    let kept: Vec<usize> = (0..all_columns.len())
        .filter(|&i| all_cells.iter().any(|row| !is_empty(&at(row, i))))
        .collect();
    let hidden: Vec<String> = (0..all_columns.len())
        .filter(|i| !kept.contains(i))
        .map(|i| escape_controls(&all_columns[i]))
        .collect();
    let names: Vec<&String> = kept.iter().map(|&i| &all_columns[i]).collect();
    let values: Vec<Vec<Value>> = all_cells
        .iter()
        .map(|row| {
            kept.iter()
                .map(|&i| shown(&all_columns[i], &at(row, i)))
                .collect()
        })
        .collect();
    let ids: Vec<bool> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let column: Vec<Value> = values.iter().map(|row| row[i].clone()).collect();
            is_id_column(name, &column)
        })
        .collect();
    let mut note = Vec::new();
    if !hidden.is_empty() {
        note.push(format!("(empty columns hidden: {})", hidden.join(", ")));
    }
    if names.is_empty() {
        let mut lines = vec!["(empty)".to_owned()];
        lines.extend(note);
        return lines.join("\n");
    }
    let texts: Vec<Vec<String>> = values
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, v)| {
                    if ids[i] {
                        cell(v)
                    } else {
                        cut(&cell(v), CELL_MAX)
                    }
                })
                .collect()
        })
        .collect();
    let heads: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if ids[i] {
                escape_controls(n)
            } else {
                cut(&escape_controls(n), CELL_MAX)
            }
        })
        .collect();
    let widths: Vec<usize> = (0..names.len())
        .map(|i| {
            texts
                .iter()
                .map(|row| width(&row[i]))
                .chain([width(&heads[i])])
                .max()
                .unwrap_or(0)
        })
        .collect();
    if let Some(max) = columns_max.filter(|&m| m > 0) {
        let id_widths: Vec<usize> = (0..widths.len())
            .filter(|&i| ids[i])
            .map(|i| widths[i])
            .collect();
        let id_width = id_widths.iter().sum::<usize>() + 2 * id_widths.len().saturating_sub(1);
        if id_width * 2 > max {
            let mut lines = vertical(&heads, &texts);
            lines.extend(note);
            return lines.join("\n");
        }
    }
    // Keep the first columns that fit the terminal; the rest is in --json.
    let mut count = names.len();
    if let Some(max) = columns_max.filter(|&m| m > 0) {
        let mut used = 0;
        count = 0;
        for w in &widths {
            let next = used + if count > 0 { 2 } else { 0 } + w;
            if next > max && count > 0 {
                break;
            }
            used = next;
            count += 1;
        }
    }
    let line = |cells: &[String]| {
        cells
            .iter()
            .take(count)
            .enumerate()
            .map(|(i, text)| format!("{text}{}", " ".repeat(widths[i] - width(text))))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };
    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    let mut lines = vec![line(&heads), line(&rule)];
    lines.extend(texts.iter().map(|row| line(row)));
    if count < names.len() {
        let rest: Vec<String> = heads[count..].to_vec();
        lines.push(format!(
            "(+{} columns did not fit: {}; all of them with --json)",
            names.len() - count,
            rest.join(", ")
        ));
    }
    lines.extend(note);
    lines.join("\n")
}

/// Each row as a block of `field  value` lines, a blank line between.
fn vertical(heads: &[String], rows: &[Vec<String>]) -> Vec<String> {
    let pad = heads.iter().map(|h| width(h)).max().unwrap_or(0);
    let mut lines = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        if n > 0 {
            lines.push(String::new());
        }
        for (i, text) in row.iter().enumerate() {
            if text.is_empty() {
                continue;
            }
            lines.push(format!(
                "{}{}  {text}",
                heads[i],
                " ".repeat(pad - width(&heads[i]))
            ));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn rows_become_a_table() {
        let data = json!([
            { "id": 1, "title": "Görev", "note": null, "tags": [{ "name": "a" }, { "name": "b" }] },
            { "id": 22, "title": "İkinci", "note": "", "tags": [] }
        ]);
        assert_eq!(
            format_data(&data, None),
            "id  title   tags\n--  ------  ----\n1   Görev   a, b\n22  İkinci  [0]\n(empty columns hidden: note)"
        );
    }

    #[test]
    fn narrow_terminal_drops_columns() {
        let data = json!([{ "id": 1, "title": "abcdefghij", "state": "open" }]);
        assert_eq!(
            format_data(&data, Some(16)),
            "id  title\n--  ----------\n1   abcdefghij\n(+1 columns did not fit: state; all of them with --json)"
        );
    }

    #[test]
    fn wide_ids_go_vertical() {
        let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let data = json!([{ "workId": id, "title": "x" }]);
        assert_eq!(
            format_data(&data, Some(60)),
            format!("workId  {id}\ntitle   x")
        );
    }

    #[test]
    fn grid_and_fields() {
        let data = json!({
            "total": 2, "durationMs": 454_558_943, "columns": ["a", "b"], "rows": [[1, "x"], [2, null]],
            "empty": []
        });
        assert_eq!(
            format_data(&data, None),
            "total: 2\ndurationMs: 5 d 6 h\nempty: (empty)\na  b\n-  -\n1  x\n2"
        );
    }

    #[test]
    fn cells_are_cut_and_scalars_flat() {
        let long = "x".repeat(50);
        let out = format_data(&json!([{ "t": long }]), None);
        assert!(out.ends_with(&format!("{}…", "x".repeat(39))), "{out}");
        assert_eq!(format_data(&json!([]), None), "(empty)");
        assert_eq!(format_data(&json!([1, "a\nb"]), None), "- 1\n- a b");
        assert_eq!(format_data(&json!("a\u{1b}b"), None), "a\\x1bb");
        assert_eq!(human_duration(950.0), "950 ms");
        assert_eq!(human_duration(61_000.0), "1 min 1 s");
    }
}
