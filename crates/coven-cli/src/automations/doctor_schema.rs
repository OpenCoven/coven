//! Bounded observations of explicitly known schema object forms.
//! Matching these forms is not a whole-store integrity or authority verdict.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Result};
use rusqlite::{params, types::ValueRef, Connection};
use serde::Deserialize;
use serde_json::{json, Map, Value};

const MAX_TEXT_BYTES: usize = 32_768;
const MAX_ROWS: usize = 256;
const MAX_FIELDS: usize = 16;
const MAX_IDENTIFIER_BYTES: usize = 128;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Forms {
    schema_version: String,
    metadata_recipe: String,
    capture_sha256: String,
    fixture_forms: Vec<String>,
    objects: BTreeMap<String, BTreeSet<String>>,
    known_indexes: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Default)]
pub(super) struct Observation {
    pub missing: u64,
    pub unrecognized: u64,
    pub incomplete: u64,
    pub unexpected: u64,
}

fn forms(required: &[(&str, &str)]) -> Result<Forms> {
    let bytes = include_bytes!("doctor-schema-forms.json");
    ensure!(bytes.len() <= 65_536, "invalid schema forms contract");
    let forms: Forms = serde_json::from_slice(bytes)?;
    ensure!(
        forms.schema_version == "coven.automations.doctor-schema-forms.v1"
            && forms.metadata_recipe == "coven.automations.doctor-schema-object.v1"
            && forms.fixture_forms.len() == 4
            && forms.capture_sha256.len() == 64,
        "invalid schema forms contract"
    );
    let keys = required
        .iter()
        .map(|(kind, name)| format!("{kind}:{name}"))
        .collect::<BTreeSet<_>>();
    ensure!(
        forms.objects.keys().cloned().collect::<BTreeSet<_>>() == keys,
        "invalid schema form object set"
    );
    for signatures in forms.objects.values() {
        ensure!(
            !signatures.is_empty()
                && signatures.len() <= 4
                && signatures.iter().all(|value| value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))),
            "invalid schema form digest"
        );
    }
    for (key, indexes) in &forms.known_indexes {
        ensure!(
            key.starts_with("table:")
                && keys.contains(key)
                && indexes.len() <= MAX_ROWS
                && indexes.iter().all(|name| name.len() <= MAX_IDENTIFIER_BYTES
                    && !name.is_empty()
                    && !name.chars().any(char::is_control)),
            "invalid schema form index set"
        );
    }
    Ok(forms)
}

#[derive(Clone, Copy)]
enum Projection {
    Definition,
    Columns,
    ForeignKeys,
    Properties,
    Indexes,
    IndexColumns,
}

impl Projection {
    // The recipe fixes field types/order, NULL values, row ordering and byte
    // sentinels. Prefixes exceeding a field limit are never hashed.
    fn sql(self) -> &'static str {
        match self {
            Self::Definition => "SELECT CAST(substr(CAST(type AS BLOB),1,17) AS TEXT), CAST(substr(CAST(name AS BLOB),1,129) AS TEXT), CAST(substr(CAST(tbl_name AS BLOB),1,129) AS TEXT), CAST(substr(CAST(sql AS BLOB),1,32769) AS TEXT) FROM main.sqlite_schema WHERE name=?1 ORDER BY type LIMIT 257",
            Self::Columns => "SELECT cid, CAST(substr(CAST(name AS BLOB),1,129) AS TEXT), CAST(substr(CAST(type AS BLOB),1,129) AS TEXT), \"notnull\", CAST(substr(CAST(dflt_value AS BLOB),1,32769) AS TEXT), pk, hidden FROM pragma_table_xinfo(?1,'main') ORDER BY cid LIMIT 257",
            Self::ForeignKeys => "SELECT id,seq, CAST(substr(CAST(\"table\" AS BLOB),1,129) AS TEXT), CAST(substr(CAST(\"from\" AS BLOB),1,129) AS TEXT), CAST(substr(CAST(\"to\" AS BLOB),1,129) AS TEXT), CAST(substr(CAST(on_update AS BLOB),1,33) AS TEXT), CAST(substr(CAST(on_delete AS BLOB),1,33) AS TEXT), CAST(substr(CAST(\"match\" AS BLOB),1,33) AS TEXT) FROM pragma_foreign_key_list(?1,'main') ORDER BY id,seq LIMIT 257",
            Self::Properties => "SELECT CAST(substr(CAST(schema AS BLOB),1,17) AS TEXT), CAST(substr(CAST(name AS BLOB),1,129) AS TEXT), CAST(substr(CAST(type AS BLOB),1,17) AS TEXT), ncol,wr,strict FROM pragma_table_list(?1) WHERE schema='main' ORDER BY schema,name LIMIT 257",
            Self::Indexes => "SELECT seq, CAST(substr(CAST(name AS BLOB),1,129) AS TEXT), \"unique\", CAST(substr(CAST(origin AS BLOB),1,17) AS TEXT), partial FROM pragma_index_list(?1,'main') ORDER BY name LIMIT 257",
            Self::IndexColumns => "SELECT seqno,cid, CAST(substr(CAST(name AS BLOB),1,129) AS TEXT), \"desc\", CAST(substr(CAST(coll AS BLOB),1,129) AS TEXT), key FROM pragma_index_xinfo(?1,'main') ORDER BY seqno LIMIT 257",
        }
    }

    fn text_limit(self, column: usize) -> usize {
        match (self, column) {
            (Self::Definition, 0) | (Self::Properties, 0 | 2) | (Self::Indexes, 3) => 16,
            (Self::Definition, 3) | (Self::Columns, 4) => MAX_TEXT_BYTES,
            (Self::ForeignKeys, 5..=7) => 32,
            _ => MAX_IDENTIFIER_BYTES,
        }
    }
}

// None denotes bounded/incomplete metadata. SQL failures propagate; they are not
// silently reclassified as a successful observation or a missing object.
fn rows(conn: &Connection, projection: Projection, name: &str) -> Result<Option<Value>> {
    let mut query = conn.prepare(projection.sql())?;
    let width = query.column_count();
    if width > MAX_FIELDS {
        return Ok(None);
    }
    let mut rows = query.query([name])?;
    let mut values = Vec::new();
    while let Some(row) = rows.next()? {
        if values.len() == MAX_ROWS {
            return Ok(None);
        }
        let mut value = Vec::with_capacity(width);
        for column in 0..width {
            value.push(match row.get_ref(column)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value)
                    if (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&value) =>
                {
                    json!(value)
                }
                ValueRef::Text(bytes) if bytes.len() <= projection.text_limit(column) => {
                    let Ok(text) = std::str::from_utf8(bytes) else {
                        return Ok(None);
                    };
                    json!(text)
                }
                _ => return Ok(None),
            });
        }
        values.push(value);
    }
    Ok(Some(json!(values)))
}

fn object(
    conn: &Connection,
    kind: &str,
    name: &str,
    known_indexes: Option<&BTreeSet<String>>,
) -> Result<Option<Value>> {
    let mut object = Map::new();
    let Some(definition) = rows(conn, Projection::Definition, name)? else {
        return Ok(None);
    };
    object.insert("definition".into(), definition);
    if kind == "table" {
        for (field, projection) in [
            ("columns", Projection::Columns),
            ("foreignKeys", Projection::ForeignKeys),
            ("properties", Projection::Properties),
            ("indexes", Projection::Indexes),
        ] {
            let Some(value) = rows(conn, projection, name)? else {
                return Ok(None);
            };
            object.insert(field.into(), value);
        }
        let mut details = BTreeMap::new();
        for index in object["indexes"]
            .as_array()
            .expect("metadata rows are arrays")
        {
            let Some(name) = index.get(1).and_then(Value::as_str) else {
                return Ok(None);
            };
            if name.len() > MAX_IDENTIFIER_BYTES {
                return Ok(None);
            }
            // Unexpected names remain in index-list evidence but never become
            // new metadata queries. Only compiled, validated names are read.
            if let Some(trusted_name) = known_indexes.and_then(|names| names.get(name)) {
                let Some(value) = rows(conn, Projection::IndexColumns, trusted_name)? else {
                    return Ok(None);
                };
                details.insert(trusted_name.clone(), value);
            }
        }
        object.insert("indexDetails".into(), json!(details));
    }
    if kind == "index" {
        let Some(value) = rows(conn, Projection::IndexColumns, name)? else {
            return Ok(None);
        };
        object.insert("columns".into(), value);
    }
    Ok(Some(Value::Object(object)))
}

/// The caller owns the coherent read snapshot. Raw metadata and fingerprints
/// never leave this helper; the report receives only mutually exclusive counts.
pub(super) fn inspect(conn: &Connection, required: &[(&str, &str)]) -> Result<Observation> {
    let forms = forms(required)?;
    // Only the compiled required set supplies bind parameters. Unknown object
    // names and SQL stay inside SQLite and never become metadata queries.
    let placeholders = vec!["(?,?)"; required.len()].join(",");
    let sql = format!(
        "WITH required(kind,name) AS (VALUES {placeholders})
         SELECT COUNT(*) FROM main.sqlite_schema AS actual
         WHERE (lower(actual.name) GLOB 'automation_*'
                OR lower(actual.name) GLOB 'idx_automation_*'
                OR lower(actual.tbl_name) GLOB 'automation_*')
           AND NOT (actual.type='index' AND actual.sql IS NULL
                    AND actual.name GLOB 'sqlite_autoindex_*')
           AND NOT EXISTS (SELECT 1 FROM required
                           WHERE required.kind=actual.type AND required.name=actual.name)"
    );
    let unexpected: i64 = conn.query_row(
        &sql,
        rusqlite::params_from_iter(required.iter().flat_map(|(kind, name)| [kind, name])),
        |row| row.get(0),
    )?;
    ensure!(
        (0..=9_007_199_254_740_991).contains(&unexpected),
        "invalid schema object count"
    );
    let mut observation = Observation {
        unexpected: unexpected as u64,
        ..Observation::default()
    };
    let mut exists =
        conn.prepare("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type=?1 AND name=?2)")?;
    for (kind, name) in required {
        let present: bool = exists.query_row(params![kind, name], |row| row.get(0))?;
        if !present {
            observation.missing += 1;
            continue;
        }
        let key = format!("{kind}:{name}");
        match object(conn, kind, name, forms.known_indexes.get(&key))? {
            None => observation.incomplete += 1,
            Some(value) => {
                let digest = super::super::contract::canonical_json::sha256_digest(&value)?;
                if !forms.objects[&key].contains(&digest) {
                    observation.unrecognized += 1;
                }
            }
        }
    }
    Ok(observation)
}
