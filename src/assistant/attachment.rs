//! Files attached to the chat. Each is a query parameter named after the
//! file (`movies.csv` is `$movies`), so a query can read all of its data,
//! for example to load it into the database. Claude reads a description of
//! each file: the whole text of a small one, the first rows of a large one.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

/// Larger files are refused.
pub const MAX_BYTES: usize = 50 * 1024 * 1024;
/// At most this many files can be attached at once.
pub const MAX_FILES: usize = 10;
/// Files up to this size are shown to Claude in full; larger ones in part.
pub const FULL_TEXT_BYTES: usize = 20 * 1024;
/// How many rows of a large file Claude sees.
const SAMPLE_ROWS: usize = 8;
/// Longer values in the sample rows are cut, so a long text column can't
/// crowd out the rest.
const SAMPLE_VALUE_CHARS: usize = 120;
/// How much of a large plain text file Claude sees.
const SAMPLE_TEXT_CHARS: usize = 4000;

/// Parameters SurrealDB sets itself, which a file can't be named after.
const RESERVED: &[&str] = &[
    "access", "after", "auth", "before", "event", "input", "parent", "reference", "session", "this", "token", "value",
];

/// An attached file. Only the name is saved in the chat history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    /// The query parameter holding the file's data, without the `$`.
    #[serde(default)]
    pub param: String,
    #[serde(skip)]
    pub size: usize,
    /// e.g. "CSV, 9125 rows, columns: movieId, title"
    #[serde(skip)]
    pub description: String,
    /// What Claude reads: the whole file, or its first rows.
    #[serde(skip)]
    pub preview: String,
    /// Whether `preview` is the whole file.
    #[serde(skip)]
    pub complete: bool,
    /// The file's data, bound as `$param` when a query runs. Shared, as
    /// every answer keeps the files it was asked with.
    #[serde(skip)]
    pub data: Arc<Json>,
}

impl Attachment {
    /// Reads a file. CSV and TSV become an array of objects, one per row,
    /// with a string field per column (empty cells are left out); JSON its
    /// value; JSON Lines an array; any other text file a string.
    pub fn from_bytes(name: &str, bytes: Vec<u8>) -> Result<Attachment, String> {
        if bytes.len() > MAX_BYTES {
            return Err(too_large(name, bytes.len() as u64));
        }
        let size = bytes.len();
        let not_text =
            || format!("{name} isn't a text file. Attach text files such as CSV, JSON, SurrealQL or notes.");
        let mut text = String::from_utf8(bytes).map_err(|_| not_text())?;
        if text.contains('\0') {
            return Err(not_text());
        }
        if text.starts_with('\u{feff}') {
            text.remove(0);
        }
        if text.trim().is_empty() {
            return Err(format!("{name} is empty."));
        }

        let extension = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        let complete = size <= FULL_TEXT_BYTES;
        let (description, data, sample) = match extension.as_str() {
            "csv" => read_csv(name, &text, b',')?,
            "tsv" => read_csv(name, &text, b'\t')?,
            "json" => {
                let data: Json = serde_json::from_str(&text).map_err(|e| format!("{name} isn't valid JSON: {e}"))?;
                let description = match &data {
                    Json::Array(items) => format!("JSON, an array of {} items", items.len()),
                    _ => "JSON".to_string(),
                };
                let sample = match &data {
                    Json::Array(items) => sample_rows(items),
                    _ => cut(&text, SAMPLE_TEXT_CHARS),
                };
                (description, data, sample)
            }
            "jsonl" | "ndjson" => {
                let items = text
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| !line.trim().is_empty())
                    .map(|(i, line)| {
                        serde_json::from_str(line).map_err(|e| format!("line {} of {name} isn't valid JSON: {e}", i + 1))
                    })
                    .collect::<Result<Vec<Json>, String>>()?;
                let sample = sample_rows(&items);
                (format!("JSON Lines, an array of {} items", items.len()), Json::Array(items), sample)
            }
            _ => {
                let sample = cut(&text, SAMPLE_TEXT_CHARS);
                ("text, as one string".to_string(), Json::String(text.clone()), sample)
            }
        };
        Ok(Attachment {
            name: name.to_string(),
            param: param_name(name),
            size,
            description,
            preview: if complete { text } else { sample },
            complete,
            data: Arc::new(data),
        })
    }

    /// Whether the file's contents are here. After a restart only the names
    /// of files attached earlier are known.
    pub fn is_loaded(&self) -> bool {
        !self.preview.is_empty()
    }

    /// e.g. "movies.csv as $movies (3.2 MB)"
    pub fn label(&self) -> String {
        format!("{} as ${} ({})", self.name, self.param, size(self.size))
    }
}

/// The attached files' data, as query parameters.
pub fn params(attached: &[Attachment]) -> Json {
    Json::Object(attached.iter().map(|a| (a.param.clone(), Json::clone(&a.data))).collect())
}

/// Why a file of `size` bytes can't be attached.
pub fn too_large(name: &str, bytes: u64) -> String {
    format!("{name} is too large to attach ({}; the limit is {}).", size(bytes as usize), size(MAX_BYTES))
}

/// Adds newly picked files to those already attached: a file with the same
/// name replaces the earlier one, and files past `MAX_FILES` are refused.
/// Returns what went wrong, if anything, as one message per file.
pub fn add(attached: &mut Vec<Attachment>, picked: Vec<Result<Attachment, String>>) -> Vec<String> {
    let mut problems = Vec::new();
    for file in picked {
        match file {
            Err(e) => problems.push(e),
            Ok(mut file) => {
                if let Some(same) = attached.iter_mut().find(|a| a.name == file.name) {
                    file.param = same.param.clone();
                    *same = file;
                } else if attached.len() >= MAX_FILES {
                    problems.push(format!("{} wasn't attached: at most {MAX_FILES} files can be attached.", file.name));
                } else {
                    // movies.csv and movies.json can't both be $movies.
                    let base = file.param.clone();
                    let mut n = 2;
                    while attached.iter().any(|a| a.param == file.param) {
                        file.param = format!("{base}_{n}");
                        n += 1;
                    }
                    attached.push(file);
                }
            }
        }
    }
    problems
}

/// `movies.csv` → `movies`, `My Data-2024.json` → `my_data_2024`.
fn param_name(file: &str) -> String {
    let stem = file.rsplit_once('.').map(|(stem, _)| stem).filter(|s| !s.is_empty()).unwrap_or(file);
    let mut param = String::new();
    for c in stem.chars() {
        let c = if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' };
        if !(c == '_' && (param.is_empty() || param.ends_with('_'))) {
            param.push(c);
        }
    }
    let param = param.trim_end_matches('_').to_string();
    if param.is_empty() || param.starts_with(|c: char| c.is_ascii_digit()) || RESERVED.contains(&param.as_str()) {
        format!("file_{param}").trim_end_matches('_').to_string()
    } else {
        param
    }
}

fn read_csv(name: &str, text: &str, delimiter: u8) -> Result<(String, Json, String), String> {
    let mut reader = csv::ReaderBuilder::new().delimiter(delimiter).flexible(true).from_reader(text.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| format!("{name} isn't valid CSV: {e}"))?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| format!("{name} isn't valid CSV: {e}"))?;
        // Empty cells are left out, so they read as NONE: casts like
        // `<option<int>>` accept that, and a field set to NONE isn't stored.
        let row: Map<String, Json> = headers
            .iter()
            .zip(record.iter())
            .filter(|(_, cell)| !cell.is_empty())
            .map(|(header, cell)| (header.clone(), Json::String(cell.to_string())))
            .collect();
        rows.push(Json::Object(row));
    }
    let sample = sample_rows(&rows);
    let description = format!("CSV, an array of {} rows; columns: {}", rows.len(), headers.join(", "));
    Ok((description, Json::Array(rows), sample))
}

/// The first rows as JSON, one per line, with long values cut.
fn sample_rows(rows: &[Json]) -> String {
    fn shorten(value: &Json) -> Json {
        match value {
            Json::String(s) if s.chars().count() > SAMPLE_VALUE_CHARS => {
                Json::String(format!("{}…", s.chars().take(SAMPLE_VALUE_CHARS).collect::<String>()))
            }
            Json::Object(o) => Json::Object(o.iter().map(|(k, v)| (k.clone(), shorten(v))).collect()),
            Json::Array(a) => Json::Array(a.iter().map(shorten).collect()),
            other => other.clone(),
        }
    }
    let mut lines: Vec<String> = rows.iter().take(SAMPLE_ROWS).map(|r| shorten(r).to_string()).collect();
    if rows.len() > SAMPLE_ROWS {
        lines.push(format!("… {} more", rows.len() - SAMPLE_ROWS));
    }
    lines.join("\n")
}

fn cut(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    format!("{}\n… (cut)", text.chars().take(chars).collect::<String>())
}

/// e.g. "194 B", "13 KB", "3.2 MB"
fn size(bytes: usize) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{} KB", b.div_ceil(1024)),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_csv_file_becomes_rows_and_a_small_one_is_shown_in_full() {
        let csv = Attachment::from_bytes("Users.csv", b"userId,name\n1,Omar\n2,\n".to_vec()).unwrap();
        assert_eq!(csv.param, "users");
        assert_eq!(csv.description, "CSV, an array of 2 rows; columns: userId, name");
        assert_eq!(*csv.data, serde_json::json!([{ "userId": "1", "name": "Omar" }, { "userId": "2" }]));
        assert!(csv.complete);
        assert_eq!(csv.preview, "userId,name\n1,Omar\n2,\n");
        assert_eq!(csv.label(), "Users.csv as $users (22 B)");
    }

    #[test]
    fn a_large_file_is_shown_as_its_first_rows() {
        let mut text = String::from("id,bio\n");
        for i in 0..2000 {
            text.push_str(&format!("{i},\"{}\nsecond line\"\n", "x".repeat(200)));
        }
        let file = Attachment::from_bytes("people.csv", text.into_bytes()).unwrap();
        assert!(!file.complete);
        assert_eq!(file.data.as_array().unwrap().len(), 2000);
        let lines: Vec<&str> = file.preview.lines().collect();
        assert_eq!(lines.len(), SAMPLE_ROWS + 1, "{}", file.preview);
        assert!(lines[0].starts_with("{\"bio\":\"xxx") && lines[0].contains('…'), "{}", lines[0]);
        assert_eq!(lines[SAMPLE_ROWS], "… 1992 more");
    }

    #[test]
    fn json_json_lines_and_text() {
        let json = Attachment::from_bytes("ids.json", b"[1, 2, 3]".to_vec()).unwrap();
        assert_eq!(*json.data, serde_json::json!([1, 2, 3]));
        let lines = Attachment::from_bytes("log.ndjson", b"{\"a\":1}\n\n{\"a\":2}\n".to_vec()).unwrap();
        assert_eq!(*lines.data, serde_json::json!([{ "a": 1 }, { "a": 2 }]));
        let notes = Attachment::from_bytes("notes.md", b"only people born before 1950".to_vec()).unwrap();
        assert_eq!(*notes.data, "only people born before 1950");
        assert!(Attachment::from_bytes("bad.json", b"[1,".to_vec()).is_err());
    }

    #[test]
    fn refuses_binary_empty_and_too_large_files() {
        assert!(Attachment::from_bytes("photo.png", vec![0x89, b'P', b'N', b'G', 0xff, 0x00]).is_err());
        assert!(Attachment::from_bytes("nul.txt", b"a\0b".to_vec()).is_err());
        assert!(Attachment::from_bytes("blank.txt", b"  \n".to_vec()).is_err());
        let large = Attachment::from_bytes("big.txt", vec![b'a'; MAX_BYTES + 1]).unwrap_err();
        assert!(large.contains("too large"), "{large}");
    }

    #[test]
    fn parameter_names_are_safe() {
        assert_eq!(param_name("movie_genres.csv"), "movie_genres");
        assert_eq!(param_name("My Data-2024.json"), "my_data_2024");
        assert_eq!(param_name("2024.csv"), "file_2024");
        assert_eq!(param_name("value.csv"), "file_value");
        assert_eq!(param_name("README"), "readme");
    }

    #[test]
    fn same_name_replaces_parameters_stay_unique_and_the_count_is_limited() {
        let file = |name: &str, text: &str| Attachment::from_bytes(name, text.as_bytes().to_vec());
        let mut attached = Vec::new();
        assert_eq!(add(&mut attached, vec![file("a.csv", "x\n1"), Err("b.png isn't a text file.".into())]).len(), 1);
        assert!(add(&mut attached, vec![file("a.csv", "x\n2"), file("a.json", "{}")]).is_empty());
        assert_eq!(attached.len(), 2);
        assert_eq!(*attached[0].data, serde_json::json!([{ "x": "2" }]));
        assert_eq!(attached[1].param, "a_2");

        let more: Vec<_> = (0..MAX_FILES).map(|i| file(&format!("f{i}.txt"), "x")).collect();
        let problems = add(&mut attached, more);
        assert_eq!(attached.len(), MAX_FILES);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(params(&attached)["a"], serde_json::json!([{ "x": "2" }]));
    }
}
