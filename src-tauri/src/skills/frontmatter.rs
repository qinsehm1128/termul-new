use std::collections::HashMap;

pub fn parse(raw: &str) -> Result<(HashMap<String, String>, String), String> {
    crate::skills::parse_skill_md(raw)
}
