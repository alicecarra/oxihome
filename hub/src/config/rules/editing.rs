use anyhow::bail;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

/// Turns the rule named `name` on or off in the text of `rules.toml`, keeping comments: off is
/// `enabled = false` right after the name, on removes the line (rules are on by default).
pub fn with_rule_enabled(text: &str, name: &str, enabled: bool) -> anyhow::Result<String> {
    let mut document: DocumentMut = text.parse()?;
    let rule = document
        .get_mut("rules")
        .and_then(|rules| rules.as_array_of_tables_mut())
        .and_then(|rules| {
            rules
                .iter_mut()
                .find(|rule| rule.get("name").and_then(|item| item.as_str()) == Some(name))
        })
        .ok_or_else(|| anyhow::anyhow!("no rule named \"{name}\""))?;
    if enabled {
        rule.remove("enabled");
    } else {
        rule.insert("enabled", value(false));
        let rank = |key: &str| match key {
            "name" => 0,
            "enabled" => 1,
            _ => 2,
        };
        rule.sort_values_by(|first, _, second, _| rank(first.get()).cmp(&rank(second.get())));
    }
    Ok(document.to_string())
}

/// Puts `rule` in the text of `rules.toml` in place of the rule named `previous`, keeping the
/// comments above it, or at the end when `previous` is `None`. Names stay unique.
pub fn with_rule_saved(text: &str, previous: Option<&str>, rule: Table) -> anyhow::Result<String> {
    let mut document: DocumentMut = text.parse()?;
    let name = rule
        .get("name")
        .and_then(|item| item.as_str())
        .ok_or_else(|| anyhow::anyhow!("the rule needs a name"))?
        .to_string();
    let rules = document
        .entry("rules")
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| anyhow::anyhow!("`rules` is not a list of [[rules]] tables"))?;
    let name_of = |table: &Table| {
        table
            .get("name")
            .and_then(|item| item.as_str())
            .map(String::from)
    };
    if rules
        .iter()
        .any(|table| name_of(table).as_deref() == Some(&name) && Some(name.as_str()) != previous)
    {
        bail!("there is already a rule named \"{name}\"");
    }
    match previous {
        Some(previous) => {
            let existing = rules
                .iter_mut()
                .find(|table| name_of(table).as_deref() == Some(previous))
                .ok_or_else(|| anyhow::anyhow!("no rule named \"{previous}\""))?;
            let decor = existing.decor().clone();
            let position = existing.position();
            *existing = rule;
            *existing.decor_mut() = decor;
            existing.set_position(position);
        }
        None => rules.push(rule),
    }
    Ok(document.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::rules::Rules;

    #[test]
    fn rules_are_turned_off_and_on_in_the_file() {
        let text = r#"# Lights.
[[rules]]
name = "Lamp at seven"
when = { at = "07:00" }
then = [{ toggle = { device = "a", entity = 1, channel = 0 } }]

[[rules]]
name = "Other"
when = { at = "08:00" }
then = []
"#;
        let off = with_rule_enabled(text, "Lamp at seven", false).unwrap();
        assert!(off.starts_with(
            "# Lights.\n[[rules]]\nname = \"Lamp at seven\"\nenabled = false\nwhen = "
        ));
        let rules: Rules = toml::from_str(&off).unwrap();
        assert!(!rules.all()[0].enabled);
        assert!(rules.all()[1].enabled);
        assert_eq!(
            with_rule_enabled(&off, "Lamp at seven", true).unwrap(),
            text
        );
        assert!(
            with_rule_enabled(text, "Missing", false)
                .unwrap_err()
                .to_string()
                .contains("no rule named")
        );
    }

    fn rule_table(name: &str, at: &str) -> Table {
        let mut table = Table::new();
        table.insert("name", value(name));
        let mut when = toml_edit::InlineTable::new();
        when.insert("at", at.into());
        table.insert("when", value(when));
        table.insert("then", value(toml_edit::Array::new()));
        table
    }

    #[test]
    fn rules_are_replaced_or_added_in_the_file() {
        let text = r#"# Lights.
[[rules]]
name = "Lamp at seven"
when = { at = "07:00" }
then = []

# Kept.
[[rules]]
name = "Other"
when = { at = "08:00" }
then = []
"#;
        let replaced = with_rule_saved(
            text,
            Some("Lamp at seven"),
            rule_table("Lamp at six", "06:00"),
        )
        .unwrap();
        assert!(replaced.starts_with("# Lights.\n[[rules]]\nname = \"Lamp at six\"\n"));
        assert!(replaced.contains("# Kept.\n[[rules]]\nname = \"Other\""));
        let added = with_rule_saved(&replaced, None, rule_table("New", "09:00")).unwrap();
        let rules: Rules = toml::from_str(&added).unwrap();
        let names: Vec<&str> = rules.all().iter().map(|rule| rule.name.as_str()).collect();
        assert_eq!(names, ["Lamp at six", "Other", "New"]);
        assert!(
            with_rule_saved(text, None, rule_table("Other", "09:00"))
                .unwrap_err()
                .to_string()
                .contains("already a rule named")
        );
        let first = with_rule_saved("", None, rule_table("First", "09:00")).unwrap();
        assert_eq!(toml::from_str::<Rules>(&first).unwrap().len(), 1);
    }
}
