//! Research wizard and the sidebar cards for a running investigation.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use charlotte::{
    ResearchNode, ask_clarifying_questions, parse_clarifying_questions, run_research_graph,
};
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{App, ClickEvent, Context, Entity, SharedString, Window, div, px};
use runebook::{FolderId, NoteId, NoteKind, VaultIndex, decode_note, encode_note};
use superglue::cancel::CancellationToken;

use super::EditorPane;
use crate::{Root, on_root};

struct QuestionField {
    prompt: String,
    answer: Entity<InputState>,
}

struct Wizard {
    query: Entity<InputState>,
    asked: String,
    questions: Vec<QuestionField>,
}

struct Job {
    note: NoteId,
    query: String,
    answers: Vec<(String, String)>,
    phase: Arc<Mutex<ResearchNode>>,
    started: Option<Instant>,
    cancel: CancellationToken,
    running: bool,
}

enum RunEnd {
    Findings(String),
    Stopped,
    Failed(String),
}

pub(super) struct ResearchDesk {
    wizard: Option<Wizard>,
    asking: bool,
    epoch: u64,
    jobs: Vec<Job>,
}

impl ResearchDesk {
    pub(super) fn new() -> Self {
        Self {
            wizard: None,
            asking: false,
            epoch: 0,
            jobs: Vec::new(),
        }
    }

    pub(super) fn wizard_open(&self) -> bool {
        self.wizard.is_some()
    }
}

impl EditorPane {
    pub(super) fn open_research_wizard(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.settings_open = false;
        self.research.asking = false;
        self.research.epoch = self.research.epoch.wrapping_add(1);
        let query = cx.new(|cx| InputState::new(window, cx));
        self.research.wizard = Some(Wizard {
            query,
            asked: String::new(),
            questions: Vec::new(),
        });
        self.notice = None;
    }

    pub(super) fn cancel_research_wizard(&mut self) {
        self.research.asking = false;
        self.research.epoch = self.research.epoch.wrapping_add(1);
        self.research.wizard = None;
    }

    pub(super) fn submit_research_query(&mut self, cx: &mut Context<Root>) {
        let Some(wizard) = &self.research.wizard else {
            return;
        };
        if self.research.asking {
            return;
        }
        let query = wizard.query.read(cx).value().trim().to_string();
        if query.is_empty() {
            self.notice = Some("Enter a question.".to_string());
            return;
        }
        self.research.asking = true;
        self.research.epoch = self.research.epoch.wrapping_add(1);
        self.notice = None;
        let epoch = self.research.epoch;
        let vault_epoch = self.epoch;
        let cancel = CancellationToken::new();
        let asked = query.clone();
        cx.spawn(async move |this, cx| {
            let task =
                runtime().spawn(async move { ask_clarifying_questions(&asked, cancel).await });
            let raw = task.await.unwrap_or(Err(charlotte::CharlotteError::msg(
                "the question call stopped",
            )));
            this.update_in(cx, |root, window, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if !pane.same_epoch(vault_epoch) || pane.research.epoch != epoch {
                    return;
                }
                pane.research.asking = false;
                match raw {
                    Ok(text) => {
                        let questions = parse_clarifying_questions(&text);
                        if questions.is_empty() {
                            pane.begin_research(query, Vec::new(), window, cx);
                        } else {
                            pane.show_research_questions(query, questions, window, cx);
                        }
                    }
                    Err(err) => {
                        pane.notice = Some(redact_research(err.to_string()));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn submit_research_answers(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let Some(wizard) = &self.research.wizard else {
            return;
        };
        let query = if wizard.asked.is_empty() {
            wizard.query.read(cx).value().trim().to_string()
        } else {
            wizard.asked.clone()
        };
        let answers = wizard
            .questions
            .iter()
            .map(|question| {
                (
                    question.prompt.clone(),
                    question.answer.read(cx).value().trim().to_string(),
                )
            })
            .collect();
        self.begin_research(query, answers, window, cx);
    }

    pub(super) fn open_research_note(
        &mut self,
        id: NoteId,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if let Err(err) = self.load_note(id, window, cx) {
            self.notice = Some(err);
        }
    }

    pub(super) fn stop_research(&mut self, id: &NoteId) {
        let Some(job) = self.research.jobs.iter().find(|job| &job.note == id) else {
            return;
        };
        if job.running {
            job.cancel.cancel();
            return;
        }
        self.research.jobs.retain(|job| &job.note != id);
    }

    fn show_research_questions(
        &mut self,
        query: String,
        questions: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let Some(wizard) = &mut self.research.wizard else {
            return;
        };
        wizard.asked = query;
        wizard.questions = questions
            .into_iter()
            .map(|prompt| QuestionField {
                prompt,
                answer: cx.new(|cx| InputState::new(window, cx)),
            })
            .collect();
    }

    fn begin_research(
        &mut self,
        query: String,
        answers: Vec<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let parent = self.parent_folder();
        let id = match unique_research_id(&self.index, &parent, &query) {
            Ok(id) => id,
            Err(err) => {
                self.notice = Some(err);
                return;
            }
        };
        let body = research_note_body(id.name(), &query, &answers);
        let file = encode_note(NoteKind::Research, &body);
        if let Err(err) = self.create_and_open(id.clone(), &file, window, cx) {
            self.notice = Some(err);
            return;
        }
        self.research.wizard = None;
        self.research.asking = false;
        self.research.jobs.push(Job {
            note: id,
            query,
            answers,
            phase: Arc::new(Mutex::new(ResearchNode::Plan)),
            started: None,
            cancel: CancellationToken::new(),
            running: false,
        });
        self.pump_research(cx);
    }

    fn pump_research(&mut self, cx: &mut Context<Root>) {
        if self.research.jobs.iter().any(|job| job.running) {
            return;
        }
        let Some(index) = self.research.jobs.iter().position(|job| !job.running) else {
            return;
        };
        let job = &mut self.research.jobs[index];
        job.running = true;
        job.started = Some(Instant::now());
        let note = job.note.clone();
        let query = job.query.clone();
        let answers = job.answers.clone();
        let cancel = job.cancel.clone();
        let phase = Arc::clone(&job.phase);
        let vault_epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let task = runtime().spawn(async move {
                let end = run_research_graph(&query, &answers, cancel.clone(), {
                    let phase = Arc::clone(&phase);
                    move |node| {
                        *phase.lock().unwrap_or_else(|err| err.into_inner()) = node;
                    }
                })
                .await;
                match end {
                    Ok(_) if cancel.is_cancelled() => RunEnd::Stopped,
                    Ok(text) => RunEnd::Findings(text),
                    Err(_) if cancel.is_cancelled() => RunEnd::Stopped,
                    Err(err) => RunEnd::Failed(redact_research(err.to_string())),
                }
            });
            loop {
                if task.is_finished() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
                this.update(cx, |root, cx| {
                    if let Some(pane) = root.pane.as_mut()
                        && pane.same_epoch(vault_epoch)
                    {
                        cx.notify();
                    }
                })
                .ok();
            }
            let end = task
                .await
                .unwrap_or(RunEnd::Failed("the investigation stopped".to_string()));
            this.update_in(cx, |root, window, cx| {
                let Some(pane) = root.pane.as_mut() else {
                    return;
                };
                if pane.same_epoch(vault_epoch) {
                    pane.finish_research(&note, end, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn finish_research(
        &mut self,
        id: &NoteId,
        end: RunEnd,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        self.research.jobs.retain(|job| &job.note != id);
        match end {
            RunEnd::Findings(text) => {
                self.write_research_block(id, &format!("## Findings\n\n{text}"), window, cx);
            }
            RunEnd::Failed(text) => {
                self.write_research_block(
                    id,
                    &format!("The investigation stopped.\n\n{text}"),
                    window,
                    cx,
                );
            }
            RunEnd::Stopped => {}
        }
        self.pump_research(cx);
    }

    fn write_research_block(
        &mut self,
        id: &NoteId,
        block: &str,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let (kind, mut body) = if self.open_note.as_ref() == Some(id) {
            (self.open_kind, self.latest.clone())
        } else {
            let Ok(file) = self.vault.read(id) else {
                return;
            };
            let (kind, body) = decode_note(&file);
            (kind, body.to_string())
        };
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push('\n');
        body.push_str(block.trim());
        body.push('\n');
        let file = encode_note(kind, &body);
        if self.vault.write(id, &file).is_err() {
            return;
        }
        if self.open_note.as_ref() == Some(id) {
            self.apply_file(&file, window, cx);
        }
    }
}

pub(super) fn wizard(
    pane: &EditorPane,
    view: gpui_kit::WeakEntity<Root>,
    cx: &App,
) -> impl IntoElement {
    let Some(wizard) = &pane.research.wizard else {
        return div().into_any_element();
    };
    let muted = cx.theme().muted_foreground;
    let asking = pane.research.asking;
    let questions = !wizard.questions.is_empty();
    let query = wizard.query.clone();
    let asked = wizard.asked.clone();
    let fields: Vec<_> = wizard
        .questions
        .iter()
        .enumerate()
        .map(|(index, question)| question_field(index, question).into_any_element())
        .collect();
    div()
        .id("research-wizard")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p_4()
        .child(
            div()
                .w_full()
                .max_w(px(560.))
                .flex()
                .flex_col()
                .gap_3()
                .child("Research rune")
                .child(div().text_sm().text_color(muted).child(if asking {
                    "Preparing questions."
                } else if questions {
                    "Answer the questions, then start the investigation."
                } else {
                    "What should this rune investigate?"
                }))
                .when(!questions, |column| {
                    column.child(Input::new(&query).w_full())
                })
                .when(questions, |column| column.child(asked))
                .children(fields)
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(if questions {
                            Button::new("research-start")
                                .small()
                                .cursor_pointer()
                                .label("Start")
                                .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
                                    if let Some(pane) = this.pane.as_mut() {
                                        pane.submit_research_answers(window, cx);
                                    }
                                    cx.notify();
                                }))
                                .into_any_element()
                        } else {
                            Button::new("research-continue")
                                .small()
                                .cursor_pointer()
                                .disabled(asking)
                                .label(if asking { "Preparing" } else { "Continue" })
                                .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                                    if let Some(pane) = this.pane.as_mut() {
                                        pane.submit_research_query(cx);
                                    }
                                    cx.notify();
                                }))
                                .into_any_element()
                        })
                        .child(
                            Button::new("research-cancel")
                                .ghost()
                                .small()
                                .cursor_pointer()
                                .label("Cancel")
                                .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                                    if let Some(pane) = this.pane.as_mut() {
                                        pane.cancel_research_wizard();
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
        )
        .into_any_element()
}

fn question_field(index: usize, question: &QuestionField) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(question.prompt.clone())
        .child(
            Input::new(&question.answer)
                .id(SharedString::from(format!("research-answer-{index}")))
                .w_full(),
        )
}

pub(super) fn cards(
    pane: &EditorPane,
    view: gpui_kit::WeakEntity<Root>,
    cx: &App,
) -> impl IntoElement {
    let border = cx.theme().sidebar_border;
    let muted = cx.theme().muted_foreground;
    div()
        .w_full()
        .flex_shrink_0()
        .when(!pane.research.jobs.is_empty(), |row| {
            row.border_t_1().border_color(border)
        })
        .children(
            pane.research
                .jobs
                .iter()
                .map(|job| research_card(job, muted, view.clone(), cx)),
        )
}

fn research_card(
    job: &Job,
    muted: impl Into<gpui_kit::Hsla> + Clone,
    view: gpui_kit::WeakEntity<Root>,
    _cx: &App,
) -> impl IntoElement {
    let id = job.note.clone();
    let open_id = id.clone();
    let stop_id = id.clone();
    let name = job.note.name().to_string();
    let phase = card_phase(job);
    let time = if job.running {
        job.started
            .map(elapsed_text)
            .unwrap_or_else(|| "0:00".to_string())
    } else {
        "Waiting".to_string()
    };
    let key = format!("research-{}", job.note);
    div()
        .id(SharedString::from(key.clone()))
        .w_full()
        .px_2()
        .py_1()
        .flex()
        .flex_col()
        .gap(px(2.))
        .cursor_pointer()
        .on_click(on_root(&view, move |this, _: &ClickEvent, window, cx| {
            if let Some(pane) = this.pane.as_mut() {
                pane.open_research_note(open_id.clone(), window, cx);
            }
            cx.notify();
        }))
        .child(name)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(muted)
                .child(phase)
                .child(time)
                .child(
                    Button::new(SharedString::from(format!("stop-{key}")))
                        .ghost()
                        .small()
                        .cursor_pointer()
                        .label("Stop")
                        .on_click(on_root(&view, move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            if let Some(pane) = this.pane.as_mut() {
                                pane.stop_research(&stop_id);
                            }
                            cx.notify();
                        })),
                ),
        )
}

fn card_phase(job: &Job) -> String {
    if !job.running {
        return "Waiting".to_string();
    }
    job.phase
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .phase()
        .to_string()
}

fn elapsed_text(started: Instant) -> String {
    let secs = started.elapsed().as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn research_note_body(name: &str, query: &str, answers: &[(String, String)]) -> String {
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

fn unique_research_id(
    index: &VaultIndex,
    parent: &FolderId,
    query: &str,
) -> Result<NoteId, String> {
    let base = research_slug(query);
    for n in 0..100u32 {
        let name = if n == 0 {
            base.clone()
        } else {
            format!("{base}-{}", n + 1)
        };
        let id = parent.join_note(&name).map_err(|err| err.to_string())?;
        if !index.contains(&id) {
            return Ok(id);
        }
    }
    Err("The folder already has these research notes.".to_string())
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

fn redact_research(text: String) -> String {
    let mut text = text;
    for key in ["OPENAI_API_KEY", "GROQ_API_KEY"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if value.len() >= 8 {
                text = text.replace(value, "[redacted]");
            }
        }
    }
    let mut out = String::new();
    for word in text.split_whitespace() {
        if word.contains('/') || word.contains('\\') {
            out.push_str("[redacted]");
        } else {
            out.push_str(word);
        }
        out.push(' ');
    }
    let trimmed = out.trim();
    let mut short = String::new();
    for (index, ch) in trimmed.chars().enumerate() {
        if index == 180 {
            break;
        }
        short.push(ch);
    }
    if short.is_empty() {
        "The investigation stopped.".to_string()
    } else {
        short
    }
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("runebook-research")
            .enable_all()
            .build()
            .expect("research runtime")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_phase_is_the_node_while_the_job_runs() {
        let job = Job {
            note: NoteId::parse("research").expect("id"),
            query: String::new(),
            answers: Vec::new(),
            phase: Arc::new(Mutex::new(ResearchNode::Search)),
            started: None,
            cancel: CancellationToken::new(),
            running: true,
        };
        assert_eq!(card_phase(&job), "Searching");
        let waiting = Job {
            running: false,
            ..job
        };
        assert_eq!(card_phase(&waiting), "Waiting");
    }

    #[test]
    fn slug_keeps_letters_and_drops_the_rest() {
        assert_eq!(
            research_slug("How did the Port change?"),
            "how-did-the-port-change"
        );
        assert_eq!(research_slug("..."), "research");
    }

    #[test]
    fn a_bad_question_payload_is_not_stored_in_the_body() {
        assert!(parse_clarifying_questions("nope").is_empty());
        let body = research_note_body("ports", "How did the port change?", &[]);
        assert!(body.contains("## Query"));
        assert!(!body.contains("## Answers"));
    }
}
