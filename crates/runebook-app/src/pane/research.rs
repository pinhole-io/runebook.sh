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
use crate::locale;
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
    plan: Arc<Mutex<String>>,
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
    fn research_chat(
        &self,
        cancel: CancellationToken,
    ) -> Result<superglue::chat::ChatOptions, charlotte::CharlotteError> {
        let Some(key) = crate::models::resolve_openai_key(&self.models.openai_api_key) else {
            return Err(charlotte::CharlotteError::msg(locale::text(
                "research.openai_key",
            )));
        };
        charlotte::research_chat_options_from(
            &key,
            &crate::models::resolve_openai_base(&self.models.openai_base_url),
            &crate::models::resolve_openai_model(&self.models.openai_model),
            self.models.tool_rounds,
            Some(cancel),
        )
    }

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
            self.notice = Some(locale::text("research.enter_question").to_string());
            return;
        }
        let cancel = CancellationToken::new();
        let options = match self.research_chat(cancel) {
            Ok(options) => options,
            Err(err) => {
                self.notice = Some(self.redact_notice(&err.to_string()));
                return;
            }
        };
        self.research.asking = true;
        self.research.epoch = self.research.epoch.wrapping_add(1);
        self.notice = None;
        let epoch = self.research.epoch;
        let vault_epoch = self.epoch;
        let asked = query.clone();
        let secrets = self.saved_secrets();
        cx.spawn(async move |this, cx| {
            let task =
                runtime().spawn(async move { ask_clarifying_questions(&asked, &options).await });
            let raw = task
                .await
                .unwrap_or(Err(charlotte::CharlotteError::msg(locale::text(
                    "research.question_stopped",
                ))));
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
                        pane.notice = Some(redact_research(err.to_string(), &secrets));
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
            plan: Arc::new(Mutex::new(String::new())),
            started: None,
            cancel: CancellationToken::new(),
            running: false,
        });
        self.pump_research(window, cx);
    }

    fn pump_research(&mut self, window: &mut Window, cx: &mut Context<Root>) {
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
        let plan = Arc::clone(&job.plan);
        let vault_epoch = self.epoch;
        let secrets = self.saved_secrets();
        let rounds = self.models.tool_rounds;
        let options = match self.research_chat(cancel.clone()) {
            Ok(options) => options,
            Err(err) => {
                let message = redact_research(err.to_string(), &secrets);
                self.finish_research(&note, RunEnd::Failed(message), window, cx);
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let task = runtime().spawn(async move {
                let end = run_research_graph(
                    &query,
                    &answers,
                    cancel.clone(),
                    options,
                    rounds,
                    {
                        let phase = Arc::clone(&phase);
                        move |node| {
                            *phase.lock().unwrap_or_else(|err| err.into_inner()) = node;
                        }
                    },
                    {
                        let plan = Arc::clone(&plan);
                        move |text| {
                            *plan.lock().unwrap_or_else(|err| err.into_inner()) = text.to_string();
                        }
                    },
                )
                .await;
                match end {
                    Ok(_) if cancel.is_cancelled() => RunEnd::Stopped,
                    Ok(text) => RunEnd::Findings(text),
                    Err(_) if cancel.is_cancelled() => RunEnd::Stopped,
                    Err(err) => RunEnd::Failed(redact_research(err.to_string(), &secrets)),
                }
            });
            loop {
                if task.is_finished() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
                this.update_in(cx, |root, window, cx| {
                    if let Some(pane) = root.pane.as_mut()
                        && pane.same_epoch(vault_epoch)
                    {
                        pane.publish_research_plan(&note, window, cx);
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
        self.publish_research_plan(id, window, cx);
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
        self.pump_research(window, cx);
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

    fn publish_research_plan(&mut self, id: &NoteId, window: &mut Window, cx: &mut Context<Root>) {
        let Some(plan) = self
            .research
            .jobs
            .iter()
            .find(|job| &job.note == id)
            .map(|job| {
                job.plan
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .clone()
            })
        else {
            return;
        };
        let plan = plan.trim();
        if plan.is_empty() {
            return;
        }
        let body = self.research_body(id);
        if parse_research_body(&body).plan == plan {
            return;
        }
        self.replace_research_body(id, &upsert_plan(&body, plan), window, cx);
    }

    fn research_body(&self, id: &NoteId) -> String {
        if self.open_note.as_ref() == Some(id) {
            return self.latest.clone();
        }
        self.vault
            .read(id)
            .map(|file| decode_note(&file).1.to_string())
            .unwrap_or_default()
    }

    fn replace_research_body(
        &mut self,
        id: &NoteId,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let kind = if self.open_note.as_ref() == Some(id) {
            self.open_kind
        } else {
            let Ok(file) = self.vault.read(id) else {
                return;
            };
            decode_note(&file).0
        };
        let file = encode_note(kind, body);
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
    let background = cx.theme().background;
    let foreground = cx.theme().foreground;
    div()
        .id("research-wizard")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .bg(background)
        .text_color(foreground)
        .p_4()
        .child(
            div()
                .w_full()
                .max_w(px(560.))
                .flex()
                .flex_col()
                .gap_3()
                .child(locale::text("research.title"))
                .child(div().text_base().text_color(muted).child(if asking {
                    locale::text("research.preparing_questions")
                } else if questions {
                    locale::text("research.answer_then_start")
                } else {
                    locale::text("research.ask")
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
                                .label(locale::text("research.start"))
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
                                .label(if asking {
                                    locale::text("research.preparing")
                                } else {
                                    locale::text("research.continue")
                                })
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
                                .label(locale::text("research.cancel"))
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

pub(super) fn report(
    pane: &EditorPane,
    view: gpui_kit::WeakEntity<Root>,
    cx: &App,
) -> impl IntoElement {
    let parsed = parse_research_body(&pane.latest);
    let border = cx.theme().border;
    let muted = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    let fill = cx.theme().sidebar_accent;
    let fill_text = cx.theme().sidebar_accent_foreground;
    let job = pane
        .research
        .jobs
        .iter()
        .find(|job| pane.open_note.as_ref() == Some(&job.note));
    let images = pane
        .open_note
        .as_ref()
        .map(|note| crate::markdown_view::ImageRoots {
            vault: pane.vault.clone(),
            note: note.clone(),
        });
    let plan_images = images.clone();
    let plan_view = view.clone();
    let show_query = !parsed.query.is_empty();
    let query = parsed.query;
    let show_answers = !parsed.answers.is_empty();
    let answers = parsed.answers;
    let show_plan = !parsed.plan.is_empty();
    let plan = parsed.plan;
    let show_error = !parsed.error.is_empty();
    let error = parsed.error;
    let findings = parsed.findings;
    let show_findings = !findings.is_empty();
    div()
        .id("research-report")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p_3()
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_3()
                .when_some(job, |column, job| {
                    let phase = card_phase(job);
                    let time = if job.running {
                        job.started
                            .map(elapsed_text)
                            .unwrap_or_else(|| "0:00".to_string())
                    } else {
                        locale::text("research.waiting").to_string()
                    };
                    column.child(
                        div()
                            .w_full()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .flex()
                            .gap_2()
                            .text_base()
                            .child(phase)
                            .child(time),
                    )
                })
                .when(show_query, |column| {
                    column.child(report_block(
                        locale::text("research.query"),
                        border,
                        muted,
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .text_base()
                            .whitespace_normal()
                            .child(query),
                    ))
                })
                .when(show_answers, |column| {
                    column.child(report_block(
                        locale::text("research.answers"),
                        border,
                        muted,
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(answers.into_iter().map(|(question, answer)| {
                                answer_row(question, answer, fill, fill_text).into_any_element()
                            })),
                    ))
                })
                .when(show_plan, |column| {
                    column.child(report_block(
                        locale::text("research.plan"),
                        border,
                        muted,
                        crate::markdown_view::view("research-plan", plan, plan_images, false)
                            .w_full()
                            .selectable(false)
                            .on_link_click(move |url, _, window, cx| {
                                let url = url.to_string();
                                plan_view
                                    .update(cx, |this, cx| {
                                        this.follow_link(&url, window, cx);
                                    })
                                    .ok();
                            }),
                    ))
                })
                .when(show_error, |column| {
                    column.child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .p_3()
                            .rounded_md()
                            .border_1()
                            .border_color(danger)
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .text_color(danger)
                                    .child(locale::text("research.stopped")),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w(px(0.0))
                                    .whitespace_normal()
                                    .text_color(danger)
                                    .child(error),
                            ),
                    )
                })
                .when(show_findings, |column| {
                    column.child(
                        div()
                            .w_full()
                            .pt_4()
                            .border_t_1()
                            .border_color(border)
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().text_base().child(locale::text("research.findings")))
                            .child(
                                crate::markdown_view::view(
                                    "research-findings",
                                    findings,
                                    images,
                                    false,
                                )
                                .w_full()
                                .selectable(false)
                                .on_link_click(
                                    move |url, _, window, cx| {
                                        let url = url.to_string();
                                        view.update(cx, |this, cx| {
                                            this.follow_link(&url, window, cx);
                                        })
                                        .ok();
                                    },
                                ),
                            ),
                    )
                }),
        )
}

fn report_block(
    label: &'static str,
    border: impl Into<gpui_kit::Hsla>,
    muted: impl Into<gpui_kit::Hsla>,
    body: impl IntoElement,
) -> impl IntoElement {
    div()
        .w_full()
        .min_w(px(0.0))
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_base().text_color(muted).child(label))
        .child(body)
}

fn answer_row(
    question: String,
    answer: String,
    fill: gpui_kit::Hsla,
    text: gpui_kit::Hsla,
) -> impl IntoElement {
    let answer_color = text;
    div()
        .w_full()
        .min_w(px(0.0))
        .p_3()
        .rounded_md()
        .bg(fill)
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .whitespace_normal()
                .text_color(text)
                .child(question),
        )
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .whitespace_normal()
                .text_base()
                .text_color(answer_color)
                .child(answer),
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
        locale::text("research.waiting").to_string()
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
                .text_base()
                .text_color(muted)
                .child(phase)
                .child(time)
                .child(
                    Button::new(SharedString::from(format!("stop-{key}")))
                        .ghost()
                        .small()
                        .cursor_pointer()
                        .label(locale::text("research.stop"))
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
        return locale::text("research.waiting").to_string();
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

struct ResearchReport {
    query: String,
    answers: Vec<(String, String)>,
    plan: String,
    findings: String,
    error: String,
}

/// Split a research note on the headings [`research_note_body`] writes.
///
/// A body with none of those headings stays the findings text.
fn parse_research_body(body: &str) -> ResearchReport {
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReportSection {
    Skip,
    Query,
    Answers,
    Plan,
    Findings,
    Error,
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

fn upsert_plan(body: &str, plan: &str) -> String {
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
    Err(locale::text("research.folder_full").to_string())
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

fn redact_research(text: String, saved: &[String]) -> String {
    let env = crate::models::env_secrets();
    let mut secrets: Vec<&str> = saved.iter().map(String::as_str).collect();
    secrets.extend(env.iter().map(String::as_str));
    let text = crate::models::redact_secrets(&text, secrets);
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
    fn a_saved_key_in_a_research_error_is_redacted() {
        let text = redact_research(
            "request failed sk-saved-key-value".to_string(),
            &["sk-saved-key-value".to_string()],
        );
        assert!(!text.contains("sk-saved-key-value"));
        assert!(text.contains("[redacted]"));
    }

    #[test]
    fn card_phase_is_the_node_while_the_job_runs() {
        let job = Job {
            note: NoteId::parse("research").expect("id"),
            query: String::new(),
            answers: Vec::new(),
            phase: Arc::new(Mutex::new(ResearchNode::Search)),
            plan: Arc::new(Mutex::new(String::new())),
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
        assert!(parse_clarifying_questions("nope").is_empty());
        let body = research_note_body("ports", "How did the port change?", &[]);
        assert!(body.contains("## Query"));
        assert!(!body.contains("## Answers"));
    }
}
