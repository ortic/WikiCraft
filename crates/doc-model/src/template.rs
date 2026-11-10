//! Template variables: `{{Customer}}` in a template's title or text is asked for when a page is
//! created from the template, and replaced by the answer.
//!
//! Text in code (code blocks, inline code) is never a variable: templates that document
//! Handlebars or Jinja keep their `{{ … }}`.

use std::collections::HashMap;

use crate::node::Node;
use crate::{marks, nodes};

/// Variables per template (more are left as they are).
pub const MAX_VARIABLES: usize = 20;
const MAX_NAME: usize = 60;
const MAX_VALUE: usize = 300;

/// `{{name}}` occurrences in `s`: (byte range of the whole `{{…}}`, trimmed name).
fn scan(s: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = s[from..].find("{{").map(|i| i + from) {
        let Some(j) = s[i + 2..].find("}}").map(|j| j + i + 2) else { break };
        let name = s[i + 2..j].trim();
        let valid = !name.is_empty() && name.chars().count() <= MAX_NAME && !name.contains(['{', '}', '\n']);
        if valid {
            out.push((i..j + 2, name));
            from = j + 2;
        } else {
            from = i + 2;
        }
    }
    out
}

fn is_code(n: &Node) -> bool {
    n.kind == nodes::CODE_BLOCK || n.marks.iter().any(|m| m.kind == marks::CODE)
}

/// Neighbouring text runs with the same marks, joined (editors and parsers split them).
fn join_runs(content: Vec<Node>) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::with_capacity(content.len());
    for c in content {
        match out.last_mut() {
            Some(prev) if prev.kind == nodes::TEXT && c.kind == nodes::TEXT && prev.marks == c.marks => {
                prev.text.get_or_insert_with(String::new).push_str(c.text.as_deref().unwrap_or(""));
            }
            _ => out.push(c),
        }
    }
    out
}

/// The variables of a template, in order of first appearance (title first).
pub fn variables(title: Option<&str>, doc: &Node) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        if out.len() < MAX_VARIABLES && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    };
    if let Some(t) = title {
        scan(t).into_iter().for_each(|(_, n)| add(n));
    }
    fn walk(n: &Node, add: &mut impl FnMut(&str)) {
        if is_code(n) {
            return;
        }
        // runs of text with the same marks are scanned as one (editors split text runs)
        let mut run = String::new();
        let mut run_marks = None;
        for c in &n.content {
            let text = (c.kind == nodes::TEXT && !is_code(c)).then(|| c.text.as_deref().unwrap_or(""));
            if text.is_none() || run_marks.is_some_and(|m| m != &c.marks) {
                scan(&run).into_iter().for_each(|(_, name)| add(name));
                run.clear();
                run_marks = None;
            }
            match text {
                Some(t) => {
                    run.push_str(t);
                    run_marks = Some(&c.marks);
                }
                None if c.kind != nodes::TEXT => walk(c, add),
                None => {}
            }
        }
        scan(&run).into_iter().for_each(|(_, name)| add(name));
    }
    walk(doc, &mut add);
    out
}

fn clean(v: &str) -> String {
    let v: String = v.split_whitespace().collect::<Vec<_>>().join(" ");
    v.chars().take(MAX_VALUE).collect()
}

/// The title with the answers filled in. Variables without an answer are left out (a title has
/// no room for hints), and so is punctuation that then trails ("Postmortem: {{Incident}}" →
/// "Postmortem").
pub fn fill_title(title: &str, values: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(title.len());
    let mut last = 0;
    for (range, name) in scan(title) {
        out.push_str(&title[last..range.start]);
        if let Some(v) = values.get(name) {
            out.push_str(&clean(v));
        }
        last = range.end;
    }
    out.push_str(&title[last..]);
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out.trim_end_matches([':', ',', '-', '–', '—', ' ']).trim().to_string()
}

/// Fills the answers into the document's text (with the text's formatting). A variable without
/// an answer becomes an instruction placeholder named after it: shown while editing, replaced
/// by typing. An empty answer removes the variable.
pub fn fill(doc: &mut Node, values: &HashMap<String, String>) {
    if is_code(doc) {
        return;
    }
    let has_var = |c: &Node| c.kind == nodes::TEXT && !is_code(c) && c.text.as_deref().is_some_and(|t| t.contains("{{"));
    if doc.content.iter().any(has_var) {
        let mut out = Vec::with_capacity(doc.content.len());
        for c in join_runs(std::mem::take(&mut doc.content)) {
            let text = match &c.text {
                Some(t) if c.kind == nodes::TEXT && !is_code(&c) && t.contains("{{") => t.clone(),
                _ => {
                    out.push(c);
                    continue;
                }
            };
            let mut last = 0;
            let mut pending = String::new();
            for (range, name) in scan(&text) {
                pending.push_str(&text[last..range.start]);
                match values.get(name) {
                    Some(v) => pending.push_str(&clean(v)),
                    None => {
                        if !pending.is_empty() {
                            out.push(Node::text_marked(std::mem::take(&mut pending), c.marks.clone()));
                        }
                        out.push(Node::new(nodes::PLACEHOLDER).attr("text", name));
                    }
                }
                last = range.end;
            }
            pending.push_str(&text[last..]);
            if !pending.is_empty() {
                out.push(Node::text_marked(pending, c.marks.clone()));
            }
        }
        doc.content = out;
    }
    for c in &mut doc.content {
        if c.kind != nodes::TEXT {
            fill(c, values);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mark;

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn finds_variables_outside_code() {
        let doc = Node::doc(vec![
            Node::paragraph(vec![Node::text("Customer: {{"), Node::text("Customer}} and {{ Owner }} again {{Customer}}")]),
            Node::new(nodes::CODE_BLOCK).with_content(vec![Node::text("{{ user.name }}")]),
            Node::paragraph(vec![Node::text_marked("{{inline}}", vec![Mark::new(marks::CODE)]), Node::text("{{}} {{a\nb}} {{Due}}")]),
        ]);
        assert_eq!(variables(Some("{{Project}}: {{Customer}}"), &doc), ["Project", "Customer", "Owner", "Due"]);
    }

    #[test]
    fn fills_text_and_keeps_formatting() {
        let mut doc = Node::doc(vec![Node::paragraph(vec![
            Node::text("For "),
            Node::text_marked("{{Customer}}", vec![Mark::new(marks::BOLD)]),
            Node::text(", owner {{Owner}}, due {{Due}}."),
        ])]);
        fill(&mut doc, &values(&[("Customer", "ACME\nCorp"), ("Due", "")]));
        let p = &doc.content[0];
        assert_eq!(p.content[1].text.as_deref(), Some("ACME Corp"));
        assert_eq!(p.content[1].marks[0].kind, marks::BOLD);
        assert_eq!(p.content[2].text.as_deref(), Some(", owner "));
        // no answer: an instruction to fill in; an empty answer: gone
        assert_eq!(p.content[3].kind, nodes::PLACEHOLDER);
        assert_eq!(p.content[3].attr_str("text"), Some("Owner"));
        assert_eq!(p.content[4].text.as_deref(), Some(", due ."));
    }

    #[test]
    fn titles() {
        let v = values(&[("Incident", "API outage")]);
        assert_eq!(fill_title("Postmortem: {{Incident}}", &v), "Postmortem: API outage");
        assert_eq!(fill_title("Postmortem: {{Incident}}", &HashMap::new()), "Postmortem");
        assert_eq!(fill_title("Welcome to the team, {{Name}}", &HashMap::new()), "Welcome to the team");
        assert_eq!(fill_title("{{A}} {{B}}", &values(&[("B", "b")])), "b");
    }
}
