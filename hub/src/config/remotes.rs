use std::collections::{BTreeMap, HashMap};

use anyhow::bail;
use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, value};

/// Infrared remotes and the codes of their keys, from `remotes.toml`:
///
/// ```toml
/// [remotes."Kit remote".keys]
/// 0 = "NEC 00 52"
/// OK = "NEC 00 1C"
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct Remotes {
    #[serde(default)]
    remotes: BTreeMap<String, Remote>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Remote {
    /// Key name to code.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}

/// A key of a remote, found by its code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteKey {
    pub remote: String,
    pub key: String,
}

impl Remotes {
    pub fn all(&self) -> &BTreeMap<String, Remote> {
        &self.remotes
    }

    pub fn code(&self, remote: &str, key: &str) -> Option<&str> {
        self.remotes.get(remote)?.keys.get(key).map(String::as_str)
    }

    pub fn key_for(&self, code: &str) -> Option<RemoteKey> {
        self.remotes.iter().find_map(|(remote, keys)| {
            keys.keys
                .iter()
                .find(|(_, known)| *known == code)
                .map(|(key, _)| RemoteKey {
                    remote: remote.clone(),
                    key: key.clone(),
                })
        })
    }

    /// The remote whose keys share the code's protocol and address (all but the command), to
    /// suggest for a new key: `RC6 00 0C` goes with the `RC6 00 ..` keys. With several, the one
    /// with the most such keys.
    pub fn remote_for(&self, code: &str) -> Option<&str> {
        let family = |code: &str| code.rsplit_once(' ').map(|(family, _)| family.to_string());
        let wanted = family(code)?;
        self.remotes
            .iter()
            .map(|(name, remote)| {
                let matching = remote
                    .keys
                    .values()
                    .filter(|known| family(known).as_ref() == Some(&wanted))
                    .count();
                (name, matching)
            })
            .filter(|(_, matching)| *matching > 0)
            .max_by_key(|(_, matching)| *matching)
            .map(|(name, _)| name.as_str())
    }

    /// A code on two keys would make rules and names ambiguous.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut seen: HashMap<&str, (&str, &str)> = HashMap::new();
        for (remote, keys) in &self.remotes {
            for (key, code) in &keys.keys {
                if let Some((other_remote, other_key)) = seen.insert(code, (remote, key)) {
                    bail!(
                        "code \"{code}\" is on {other_remote} key {other_key} and on {remote} key {key}"
                    );
                }
            }
        }
        Ok(())
    }
}

/// A code as an IR receiver decodes it: `NEC 00 45`, `NEC 1234 45`, `SONY15 30 15`, `RC5 00 0C`
/// or `RC6 00 0C`; not a held key's `repeat`, raw timings nor any other text.
pub fn is_decoded_code(text: &str) -> bool {
    let hex = |word: &str, lengths: &[usize]| {
        lengths.contains(&word.len()) && word.chars().all(|digit| digit.is_ascii_hexdigit())
    };
    match text.split(' ').collect::<Vec<_>>().as_slice() {
        [protocol, address, command] => {
            let address_lengths: &[usize] = match *protocol {
                "NEC" => &[2, 4],
                "SONY12" | "SONY15" | "RC5" | "RC6" => &[2],
                "SONY20" => &[4],
                _ => return false,
            };
            hex(address, address_lengths) && hex(command, &[2])
        }
        _ => false,
    }
}

/// `remotes.toml` with one more key, keeping the file's comments and layout.
pub fn with_key(text: &str, remote: &str, key: &str, code: &str) -> anyhow::Result<String> {
    let mut document: DocumentMut = text.parse()?;
    let remotes = document
        .entry("remotes")
        .or_insert_with(implicit_table)
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("`remotes` is not a table"))?;
    let keys = remotes
        .entry(remote)
        .or_insert_with(implicit_table)
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("remote \"{remote}\" is not a table"))?
        .entry("keys")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| anyhow::anyhow!("keys of \"{remote}\" are not a table"))?;
    keys.insert(key, value(code));
    Ok(document.to_string())
}

/// A table that only shows through its subtables, so new remotes read as `[remotes."Name".keys]`.
fn implicit_table() -> Item {
    let mut table = Table::new();
    table.set_implicit(true);
    Item::Table(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
        # The kit's 17-key remote.
        [remotes."Kit remote".keys]
        0 = "NEC 00 52"
        OK = "NEC 00 1C"
    "#;

    #[test]
    fn keys_are_found_by_code_and_by_name() {
        let remotes: Remotes = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(
            remotes.key_for("NEC 00 1C"),
            Some(RemoteKey {
                remote: "Kit remote".into(),
                key: "OK".into(),
            })
        );
        assert_eq!(remotes.key_for("NEC 00 99"), None);
        assert_eq!(remotes.code("Kit remote", "0"), Some("NEC 00 52"));
        assert_eq!(remotes.code("Kit remote", "9"), None);
    }

    #[test]
    fn new_codes_go_with_the_remote_of_the_same_protocol_and_address() {
        let remotes: Remotes = toml::from_str(
            r#"
            [remotes.Kit.keys]
            0 = "NEC 00 52"
            [remotes.Receiver.keys]
            power = "SONY15 30 15"
            [remotes.TV.keys]
            power = "RC6 00 0C"
            up = "RC6 00 58"
            [remotes.Other.keys]
            stray = "RC6 00 5A"
            "#,
        )
        .unwrap();
        assert_eq!(remotes.remote_for("RC6 00 BF"), Some("TV"));
        assert_eq!(remotes.remote_for("NEC 00 16"), Some("Kit"));
        assert_eq!(remotes.remote_for("SONY15 30 12"), Some("Receiver"));
        assert_eq!(remotes.remote_for("NEC 04 16"), None);
        assert_eq!(remotes.remote_for("RC5 00 0C"), None);
    }

    #[test]
    fn decoded_codes() {
        for code in [
            "NEC 00 45",
            "NEC 1234 45",
            "SONY15 30 15",
            "SONY20 0A1B 15",
            "RC6 00 0C",
        ] {
            assert!(is_decoded_code(code), "{code}");
        }
        for text in [
            "NEC 00 45 repeat",
            "raw 9000 4500 560",
            "Mosquitto 2.0.22",
            "NEC 0 45",
            "SONY 30 15",
            "",
        ] {
            assert!(!is_decoded_code(text), "{text}");
        }
    }

    #[test]
    fn a_code_on_two_keys_is_refused() {
        let remotes: Remotes = toml::from_str(
            r#"
            [remotes.A.keys]
            up = "NEC 00 18"
            [remotes.B.keys]
            power = "NEC 00 18"
            "#,
        )
        .unwrap();
        let error = remotes.validate().unwrap_err().to_string();
        assert!(error.contains("A key up and on B key power"), "{error}");
    }

    #[test]
    fn new_keys_keep_comments_and_new_remotes_get_their_own_table() {
        let text = with_key(EXAMPLE, "Kit remote", "up", "NEC 00 18").unwrap();
        assert!(text.contains("# The kit's 17-key remote."));
        assert!(text.contains("up = \"NEC 00 18\""));
        let text = with_key(&text, "TV", "power", "NEC 04 08").unwrap();
        assert!(text.contains("[remotes.TV.keys]"), "{text}");
        let remotes: Remotes = toml::from_str(&text).unwrap();
        assert_eq!(remotes.code("Kit remote", "up"), Some("NEC 00 18"));
        assert_eq!(remotes.code("TV", "power"), Some("NEC 04 08"));
        assert_eq!(remotes.code("Kit remote", "0"), Some("NEC 00 52"));
    }

    #[test]
    fn the_first_key_creates_the_file_content() {
        let text = with_key("", "Kit remote", "0", "NEC 00 52").unwrap();
        let remotes: Remotes = toml::from_str(&text).unwrap();
        assert_eq!(remotes.code("Kit remote", "0"), Some("NEC 00 52"));
    }
}
