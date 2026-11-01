//! Research note body: query, answers, plan, and findings.

use runebook::{FolderId, NoteId, VaultIndex};

use crate::ResearchError;

/// The sections of one research note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchReport {
    /// The question under `## Query`.
    pub query: String,
    /// Clarifying answers under `## Answers`.
    pub answers: Vec<(String, String)>,
    /// The plan under `## Plan`.
    pub plan: String,
    /// The investigation text under `## Findings`.
    pub findings: String,
    /// Stop text that is not a findings section.
    pub error: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReportSection {
    Skip,
    Query,
    Answers,
    Plan,
    Findings,
    Error,
}

/// Split a research note on the headings [`research_note_body`] writes.
///
/// A body with none of those headings stays the findings text.
#[must_use]
pub fn parse_research_body(body: &str) -> ResearchReport {
    let mut section = ReportSection::Skip;
    let mut saw_structure = false;
    let mut preamble = String::new();
    let mut query = String::new();
    let mut answers_raw = String::new();
    let mut plan = String::new();
    let mut findings = String::new();
    let mut error = String::new();
    for line in body.lines() {
        let line = line.trim_end_matches('\r');
        if section != ReportSection::Findings
            && let Some(next) = report_heading(line)
        {
            saw_structure = true;
            section = next;
            continue;
        }
        if saw_structure
            && section != ReportSection::Findings
            && line.trim() == "The investigation stopped."
        {
            section = ReportSection::Error;
            push_line(&mut error, line);
            continue;
        }
        match section {
            ReportSection::Skip => {
                if !line.trim().is_empty() && !is_slug_heading(line) {
                    push_line(&mut preamble, line);
                }
            }
            ReportSection::Query => push_line(&mut query, line),
            ReportSection::Plan => push_line(&mut plan, line),
            ReportSection::Answers if line.is_empty() || is_answer_line(line) => {
                push_line(&mut answers_raw, line);
            }
            ReportSection::Answers => {
                section = ReportSection::Error;
                push_line(&mut error, line);
            }
            ReportSection::Findings => push_line(&mut findings, line),
            ReportSection::Error => push_line(&mut error, line),
        }
    }
    if !saw_structure {
        return ResearchReport {
            query: String::new(),
            answers: Vec::new(),
            plan: String::new(),
            findings: body.trim().to_string(),
            error: String::new(),
        };
    }
    let mut error = error.trim().to_string();
    let preamble = preamble.trim();
    if !preamble.is_empty() {
        if error.is_empty() {
            error = preamble.to_string();
        } else {
            error = format!("{preamble}\n\n{error}");
        }
    }
    ResearchReport {
        query: query.trim().to_string(),
        answers: parse_answer_lines(&answers_raw),
        plan: plan.trim().to_string(),
        findings: findings.trim().to_string(),
        error,
    }
}

fn report_heading(line: &str) -> Option<ReportSection> {
    match line.trim() {
        "## Query" => Some(ReportSection::Query),
        "## Answers" => Some(ReportSection::Answers),
        "## Plan" => Some(ReportSection::Plan),
        "## Findings" => Some(ReportSection::Findings),
        _ => None,
    }
}

fn is_slug_heading(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("# ") && !trimmed.starts_with("##")
}

fn is_answer_line(line: &str) -> bool {
    line.starts_with("- ") || line.starts_with(' ') || line.starts_with('\t')
}

/// Insert or replace the `## Plan` section.
#[must_use]
pub fn upsert_plan(body: &str, plan: &str) -> String {
    let heading = "## Plan";
    let lines: Vec<&str> = body.lines().collect();
    let Some(start) = lines.iter().position(|line| line.trim() == heading) else {
        let block = format!("{heading}\n\n{plan}\n");
        if let Some(findings) = lines.iter().position(|line| line.trim() == "## Findings") {
            let mut out = join_lines(&lines[..findings]);
            out.push_str(&block);
            out.push('\n');
            out.push_str(&join_lines(&lines[findings..]));
            return out;
        }
        let mut out = body.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&block);
        return out;
    };
    let end = lines
        .iter()
        .skip(start + 1)
        .position(|line| line.trim().starts_with("## "))
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());
    let mut out = join_lines(&lines[..start]);
    out.push_str(heading);
    out.push_str("\n\n");
    out.push_str(plan.trim());
    out.push('\n');
    if end < lines.len() {
        out.push('\n');
        out.push_str(&join_lines(&lines[end..]));
    }
    out
}

fn join_lines(lines: &[&str]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn push_line(buf: &mut String, line: &str) {
    if !buf.is_empty() {
        buf.push('\n');
    }
    buf.push_str(line);
}

fn parse_answer_lines(text: &str) -> Vec<(String, String)> {
    let mut items = Vec::new();
    let mut question: Option<String> = None;
    let mut answer = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("- ") {
            if let Some(question) = question.take() {
                items.push((question, answer.trim().to_string()));
                answer.clear();
            }
            question = Some(rest.trim().to_string());
        } else if question.is_some() && !line.trim().is_empty() {
            if !answer.is_empty() {
                answer.push('\n');
            }
            answer.push_str(line.trim());
        }
    }
    if let Some(question) = question {
        items.push((question, answer.trim().to_string()));
    }
    items
}

/// Body for a new research note.
///
/// `name` is the note slug. It is the title line. It is not the query.
#[must_use]
pub fn research_note_body(name: &str, query: &str, answers: &[(String, String)]) -> String {
    let mut body = format!("# {name}\n\n## Query\n\n{query}\n");
    if answers.is_empty() {
        return body;
    }
    body.push_str("\n## Answers\n\n");
    for (question, answer) in answers {
        body.push_str(&format!("- {question}\n  {answer}\n"));
    }
    body
}

/// First free note id for `query` under `parent`.
///
/// # Errors
///
/// Returns [`ResearchError::Name`] when the slug is not a safe note id.
/// Returns [`ResearchError::FolderFull`] after one hundred taken ids.
pub fn unique_research_id(
    index: &VaultIndex,
    parent: &FolderId,
    query: &str,
) -> Result<NoteId, ResearchError> {
    let base = research_slug(query);
    for n in 0..100u32 {
        let name = if n == 0 {
            base.clone()
        } else {
            format!("{base}-{}", n + 1)
        };
        let id = parent
            .join_note(&name)
            .map_err(|err| ResearchError::Name(err.to_string()))?;
        if !index.contains(&id) {
            return Ok(id);
        }
    }
    Err(ResearchError::FolderFull)
}

fn research_slug(query: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in query.chars() {
        if out.len() >= 48 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "research".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_keeps_letters_and_drops_the_rest() {
        assert_eq!(
            research_slug("How did the Port change?"),
            "how-did-the-port-change"
        );
        assert_eq!(research_slug("..."), "research");
    }

    #[test]
    fn the_report_splits_query_answers_and_findings() {
        let mut body = research_note_body(
            "ports",
            "How did the port change?",
            &[("Which years?".into(), "1990 to 2000".into())],
        );
        body.push_str("\n## Findings\n\nThe quay moved.\n");
        let report = parse_research_body(&body);
        assert_eq!(report.query, "How did the port change?");
        assert_eq!(
            report.answers,
            vec![("Which years?".to_string(), "1990 to 2000".to_string())]
        );
        assert_eq!(report.findings, "The quay moved.");
        assert!(report.error.is_empty());
    }

    #[test]
    fn missing_findings_stay_empty() {
        let body = research_note_body("ports", "How did the port change?", &[]);
        let report = parse_research_body(&body);
        assert_eq!(report.query, "How did the port change?");
        assert!(report.findings.is_empty());
        assert!(report.error.is_empty());
    }

    #[test]
    fn a_plain_body_stays_findings() {
        let report = parse_research_body("# Title\n\nHello.\n");
        assert!(report.query.is_empty());
        assert!(report.answers.is_empty());
        assert_eq!(report.findings, "# Title\n\nHello.");
    }

    #[test]
    fn the_slug_heading_is_not_the_title() {
        let body = research_note_body("do-research-on-wow", "Do research on the mage class", &[]);
        let report = parse_research_body(&body);
        assert_eq!(report.query, "Do research on the mage class");
        assert!(!report.query.contains("do-research-on-wow"));
        assert!(!report.findings.contains("do-research-on-wow"));
    }

    #[test]
    fn the_report_keeps_the_plan() {
        let mut body = research_note_body("ports", "How did the port change?", &[]);
        body.push_str("\n## Plan\n\nSearch the quay.\n\n## Findings\n\nThe quay moved.\n");
        let report = parse_research_body(&body);
        assert_eq!(report.plan, "Search the quay.");
        assert_eq!(report.findings, "The quay moved.");
        assert_eq!(report.query, "How did the port change?");
    }

    #[test]
    fn a_revised_plan_replaces_the_old_plan() {
        let body = "# ports\n\n## Query\n\nHow?\n\n## Plan\n\nFirst.\n\n## Findings\n\nDone.\n";
        let next = upsert_plan(body, "Second.");
        let report = parse_research_body(&next);
        assert_eq!(report.plan, "Second.");
        assert_eq!(report.query, "How?");
        assert_eq!(report.findings, "Done.");
    }

    #[test]
    fn the_plan_is_inserted_before_findings() {
        let body = "# ports\n\n## Query\n\nHow?\n\n## Findings\n\nDone.\n";
        let report = parse_research_body(&upsert_plan(body, "Search."));
        assert_eq!(report.plan, "Search.");
        assert_eq!(report.findings, "Done.");
    }

    #[test]
    fn a_stop_message_is_not_findings() {
        let mut body = research_note_body("ports", "How did the port change?", &[]);
        body.push_str("\nThe investigation stopped.\n\nno key\n");
        let report = parse_research_body(&body);
        assert!(report.findings.is_empty());
        assert!(report.error.contains("The investigation stopped."));
        assert!(!report.error.contains("## Findings"));
    }

    #[test]
    fn a_bad_question_payload_is_not_stored_in_the_body() {
        assert!(charlotte::parse_clarifying_questions("nope").is_empty());
        let body = research_note_body("ports", "How did the port change?", &[]);
        assert!(body.contains("## Query"));
        assert!(!body.contains("## Answers"));
    }
}
