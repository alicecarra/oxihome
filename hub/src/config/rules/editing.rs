use toml_edit::{DocumentMut, value};

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
}
