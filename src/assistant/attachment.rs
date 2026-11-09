//! Files attached to a question. Their text goes to Claude with the
//! question, for example a list of names to filter by, CSV or JSON rows to
//! insert, or notes about what the query should do.

use serde::{Deserialize, Serialize};

/// Larger files are refused, so one question can't cost a fortune.
pub const MAX_BYTES: usize = 200 * 1024;
/// At most this many files go with one question.
pub const MAX_FILES: usize = 5;

/// An attached text file. Only the name is saved in the chat history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    #[serde(skip)]
    pub text: String,
}

impl Attachment {
    /// Reads a file's contents as an attachment. Only text (UTF-8) files of
    /// up to `MAX_BYTES` are accepted.
    pub fn from_bytes(name: &str, bytes: Vec<u8>) -> Result<Attachment, String> {
        if bytes.len() > MAX_BYTES {
            return Err(too_large(name, bytes.len() as u64));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{name} isn't a text file. Attach text files such as CSV, JSON, SurrealQL or notes."))?;
        if text.contains('\0') {
            return Err(format!("{name} isn't a text file. Attach text files such as CSV, JSON, SurrealQL or notes."));
        }
        if text.trim().is_empty() {
            return Err(format!("{name} is empty."));
        }
        Ok(Attachment { name: name.to_string(), text })
    }

    /// e.g. "people.csv (12 KB)"
    pub fn label(&self) -> String {
        let bytes = self.text.len();
        if bytes < 1024 {
            format!("{} ({bytes} B)", self.name)
        } else {
            format!("{} ({} KB)", self.name, bytes.div_ceil(1024))
        }
    }
}

/// Why a file of `size` bytes can't be attached.
pub fn too_large(name: &str, size: u64) -> String {
    format!("{name} is too large to attach ({} KB; the limit is {} KB).", size.div_ceil(1024), MAX_BYTES / 1024)
}

/// Adds newly picked files to those already attached: a file with the same
/// name replaces the earlier one, and files past `MAX_FILES` are refused.
/// Returns what went wrong, if anything, as one message per file.
pub fn add(attached: &mut Vec<Attachment>, picked: Vec<Result<Attachment, String>>) -> Vec<String> {
    let mut problems = Vec::new();
    for file in picked {
        match file {
            Err(e) => problems.push(e),
            Ok(file) => {
                if let Some(same) = attached.iter_mut().find(|a| a.name == file.name) {
                    *same = file;
                } else if attached.len() >= MAX_FILES {
                    problems.push(format!("{} wasn't attached: at most {MAX_FILES} files go with a question.", file.name));
                } else {
                    attached.push(file);
                }
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_text_and_refuses_binary_empty_and_large_files() {
        let csv = Attachment::from_bytes("people.csv", b"name\nalice\n".to_vec()).unwrap();
        assert_eq!(csv.text, "name\nalice\n");
        assert_eq!(csv.label(), "people.csv (11 B)");

        assert!(Attachment::from_bytes("photo.png", vec![0x89, b'P', b'N', b'G', 0xff, 0x00]).is_err());
        assert!(Attachment::from_bytes("nul.txt", b"a\0b".to_vec()).is_err());
        assert!(Attachment::from_bytes("blank.txt", b"  \n".to_vec()).is_err());
        let large = Attachment::from_bytes("big.json", vec![b'a'; MAX_BYTES + 1]).unwrap_err();
        assert!(large.contains("too large"), "{large}");
    }

    #[test]
    fn same_name_replaces_and_the_count_is_limited() {
        let file = |name: &str, text: &str| Ok(Attachment { name: name.into(), text: text.into() });
        let mut attached = Vec::new();
        assert!(add(&mut attached, vec![file("a.csv", "1"), Err("b.png isn't a text file.".into())]).len() == 1);
        assert!(add(&mut attached, vec![file("a.csv", "2")]).is_empty());
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].text, "2");

        let more: Vec<_> = (0..MAX_FILES).map(|i| file(&format!("{i}.txt"), "x")).collect();
        let problems = add(&mut attached, more);
        assert_eq!(attached.len(), MAX_FILES);
        assert_eq!(problems.len(), 1, "{problems:?}");
    }
}
