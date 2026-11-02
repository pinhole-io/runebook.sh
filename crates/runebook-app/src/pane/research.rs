//! Research wizard and the sidebar cards for a running investigation.

use std::time::{Duration, Instant};

use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Escape, Input, InputState};
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{App, ClickEvent, Context, Entity, SharedString, Window, div, px};
use runebook::{NoteId, NoteKind, decode_note, encode_note};
#[cfg(test)]
use runebook_research::ResearchNode;
use runebook_research::{
    InvestigateEnd, Investigation, ResearchError, chat_options, clarifying_questions,
    parse_research_body, questions_from_reply, research_note_body, runtime, store_block,
    store_plan, unique_research_id,
};
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
    run: Investigation,
    started: Option<Instant>,
    running: bool,
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
    fn research_chat(&self, run: &Investigation) -> Result<superglue::chat::ChatOptions, String> {
        let key =
            crate::models::resolve_openai_key(&self.models.openai_api_key).unwrap_or_default();
        run.prepare(
            &key,
            &crate::models::resolve_openai_base(&self.models.openai_base_url),
            &crate::models::resolve_openai_model(&self.models.openai_model),
            self.models.tool_rounds,
        )
        .map_err(|err| match err {
            ResearchError::MissingKey => locale::text("research.openai_key").to_string(),
            other => other.to_string(),
        })
    }

    pub(super) fn open_research_wizard(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.settings_open = false;
        self.tasks_open = false;
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
        let key =
            crate::models::resolve_openai_key(&self.models.openai_api_key).unwrap_or_default();
        let options = match chat_options(
            &key,
            &crate::models::resolve_openai_base(&self.models.openai_base_url),
            &crate::models::resolve_openai_model(&self.models.openai_model),
            self.models.tool_rounds,
            Some(CancellationToken::new()),
        ) {
            Ok(options) => options,
            Err(ResearchError::MissingKey) => {
                self.notice = Some(self.redact_notice(locale::text("research.openai_key")));
                return;
            }
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
            let task = runtime().spawn(async move { clarifying_questions(&asked, &options).await });
            let raw = task.await.unwrap_or(Err(ResearchError::Stopped));
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
                        let questions = questions_from_reply(&text);
                        if questions.is_empty() {
                            pane.begin_research(query, Vec::new(), window, cx);
                        } else {
                            pane.show_research_questions(query, questions, window, cx);
                        }
                    }
                    Err(err) => {
                        let text = match err {
                            ResearchError::Stopped => {
                                locale::text("research.question_stopped").to_string()
                            }
                            other => other.to_string(),
                        };
                        pane.notice = Some(redact_research(text, &secrets));
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
        let Some(job) = self.research.jobs.iter().find(|job| job.run.note() == id) else {
            return;
        };
        if job.running {
            job.run.cancel();
            return;
        }
        self.research.jobs.retain(|job| job.run.note() != id);
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
            Err(ResearchError::FolderFull) => {
                self.notice = Some(locale::text("research.folder_full").to_string());
                return;
            }
            Err(err) => {
                self.notice = Some(err.to_string());
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
            run: Investigation::start(id, query, answers),
            started: None,
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
        let note = job.run.note().clone();
        let run = job.run.clone();
        let vault_epoch = self.epoch;
        let secrets = self.saved_secrets();
        let rounds = self.models.tool_rounds;
        let options = match self.research_chat(&run) {
            Ok(options) => options,
            Err(err) => {
                let message = redact_research(err, &secrets);
                self.finish_research(&note, InvestigateEnd::Failed(message), window, cx);
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let task = runtime().spawn(async move {
                match run.run(options, rounds).await {
                    InvestigateEnd::Failed(text) => {
                        InvestigateEnd::Failed(redact_research(text, &secrets))
                    }
                    end => end,
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
            let end = task.await.unwrap_or(InvestigateEnd::Failed(
                "the investigation stopped".to_string(),
            ));
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
        end: InvestigateEnd,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        self.publish_research_plan(id, window, cx);
        self.research.jobs.retain(|job| job.run.note() != id);
        match end {
            InvestigateEnd::Findings(text) => {
                self.write_research_block(id, &format!("## Findings\n\n{text}"), window, cx);
            }
            InvestigateEnd::Failed(text) => {
                self.write_research_block(
                    id,
                    &format!("The investigation stopped.\n\n{text}"),
                    window,
                    cx,
                );
            }
            InvestigateEnd::Stopped => {}
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
        let (kind, body) = if self.open_note.as_ref() == Some(id) {
            (self.open_kind, self.latest.clone())
        } else {
            let Ok(file) = self.vault.read(id) else {
                return;
            };
            let (kind, body) = decode_note(&file);
            (kind, body.to_string())
        };
        let Ok(file) = store_block(&self.vault, id, kind, &body, block) else {
            return;
        };
        if self.open_note.as_ref() == Some(id) {
            self.apply_file(&file, window, cx);
        }
    }

    fn publish_research_plan(&mut self, id: &NoteId, window: &mut Window, cx: &mut Context<Root>) {
        let Some(plan) = self
            .research
            .jobs
            .iter()
            .find(|job| job.run.note() == id)
            .map(|job| job.run.plan_text())
        else {
            return;
        };
        let body = self.research_body(id);
        let kind = if self.open_note.as_ref() == Some(id) {
            self.open_kind
        } else {
            let Ok(file) = self.vault.read(id) else {
                return;
            };
            decode_note(&file).0
        };
        let Ok(Some(file)) = store_plan(&self.vault, id, kind, &body, &plan) else {
            return;
        };
        if self.open_note.as_ref() == Some(id) {
            self.apply_file(&file, window, cx);
        }
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
        .on_action(on_root(&view, |this, _: &Escape, _, cx| {
            cx.stop_propagation();
            if let Some(pane) = this.pane.as_mut() {
                pane.cancel_research_wizard();
            }
            cx.notify();
        }))
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
        .find(|job| pane.open_note.as_ref() == Some(job.run.note()));
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
                        crate::markdown_view::view("research-plan", plan, plan_images, false, cx)
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
                                    cx,
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
    let id = job.run.note().clone();
    let open_id = id.clone();
    let stop_id = id.clone();
    let name = job.run.note().name().to_string();
    let phase = card_phase(job);
    let time = if job.running {
        job.started
            .map(elapsed_text)
            .unwrap_or_else(|| "0:00".to_string())
    } else {
        locale::text("research.waiting").to_string()
    };
    let key = format!("research-{}", job.run.note());
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
    job.run.phase_label()
}

fn elapsed_text(started: Instant) -> String {
    let secs = started.elapsed().as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
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
        let run = Investigation::start(
            NoteId::parse("research").expect("id"),
            String::new(),
            Vec::new(),
        );
        run.set_phase(ResearchNode::Search);
        let job = Job {
            run,
            started: None,
            running: true,
        };
        assert_eq!(card_phase(&job), "Searching");
        let waiting = Job {
            running: false,
            ..job
        };
        assert_eq!(card_phase(&waiting), "Waiting");
    }
}
