//! Modern AI-Agent Workspace Views (`Tasks & Daily`, `Code & Env`, `Projects`, `llama.cpp Control`, `Custom Studio & Capri Lab`).

use crate::app::{ChannelTab, LoraDesktopApp, Message};
use crate::dashboard::{CapriAuraPreset, CapriEyeMode};
use crate::theme::{self, CyberPalette, CyberThemePreset, SurfaceDepth};
use crate::ux_studio::{compute_context_budget_usage, view_mini_bot_avatar};
use crate::workspace_db::{
    AgentKind, ExecutionAccessMode, FolderAccessLevel, KvCacheType, ReasoningEffort, TaskFilter,
    TaskKind, TaskPriority, TaskStatus,
};
use iced::alignment::Alignment;
use iced::widget::{
    button, checkbox, column, container, progress_bar, row, rule, scrollable, slider, space, text,
    text_input, Column, Row,
};
use iced::{Color, Element, Font, Length, Padding};
use lora_core::{
    allocate_context_budget, slice_virtualized_transcript, ContextBudgetRequest, TuningProfile,
};

fn section_card<'a>(
    pal: CyberPalette,
    rad: f32,
    pad: u16,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(content)
        .padding(pad)
        .width(Length::Fill)
        .style(theme::palette_card_container(
            pal.bg_card,
            pal.text_primary,
            pal.border_subtle,
            rad,
        ))
        .into()
}

fn stat_badge<'a>(label: &'a str, val: String, accent: Color, bg: Color) -> Element<'a, Message> {
    container(
        row![
            text(label).size(10).font(Font::MONOSPACE).color(accent),
            text(val).size(11).font(Font::MONOSPACE).color(accent),
        ]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([4, 9]))
    .style(theme::pill_container(
        bg,
        accent,
        Color { a: 0.45, ..accent },
    ))
    .into()
}

/// Unified Agent Controls strip shared across Chat, Code & Daily/Task views:
/// Model selector, Local Model status, Access Mode `{LOCAL | API}`, Agent Type, Reasoning Effort,
/// Folder Trust status, and live Token Context meter.
pub fn view_agent_execution_bar<'a>(app: &'a LoraDesktopApp) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let c_rad = app.dashboard.control_radius();
    let usage = compute_context_budget_usage(
        &app.messages,
        &app.staged_attachments,
        &app.composer_input,
        app.requested_context,
    );
    let trust = app
        .workspace_db
        .folder_trust_for_path(app.active_project_id, &app.workspace_root_input);

    let (local_status_lbl, local_status_col) = if app.active_run_id.is_some() {
        ("LOCAL: BUSY", pal.warning)
    } else if app.telemetry.models.is_empty() {
        ("LOCAL: NO GGUF", pal.critical)
    } else if let Some(sel) = &app.selected_model {
        let _ = sel;
        ("LOCAL: READY", pal.nominal)
    } else {
        ("LOCAL: STANDBY", pal.accent_secondary)
    };

    let mut access_row = Row::new().spacing(4).align_y(Alignment::Center);
    access_row = access_row.push(
        text("ACCESS")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    for mode in ExecutionAccessMode::ALL {
        let active = app.access_mode == mode;
        let accent = if mode == ExecutionAccessMode::Local {
            pal.nominal
        } else {
            pal.warning
        };
        access_row = access_row.push(
            button(text(mode.label()).size(10).font(Font::MONOSPACE))
                .padding(Padding::from([3, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    accent,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectAccessMode(mode)),
        );
    }

    let mut model_row = Row::new().spacing(5).align_y(Alignment::Center);
    model_row = model_row.push(
        text("MODEL")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    if app.access_mode == ExecutionAccessMode::Api {
        let key_ok = app
            .workspace_db
            .resolve_env_value(app.active_project_id, &app.api_config.api_key_env)
            .is_some();
        model_row = model_row.push(
            button(
                text(format!(
                    "API: {} ({})",
                    app.api_config.model_name,
                    if key_ok { "KEY OK" } else { "NO KEY" }
                ))
                .size(10)
                .font(Font::MONOSPACE),
            )
            .padding(Padding::from([3, 9]))
            .style(theme::cyber_chip_button_with_radius(
                true,
                if key_ok { pal.warning } else { pal.critical },
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.warning,
                c_rad,
            ))
            .on_press(Message::SelectChannel(ChannelTab::CodeWorkspace)),
        );
    } else if app.telemetry.models.is_empty() {
        model_row = model_row.push(
            button(text("Import GGUF ->").size(10))
                .padding(Padding::from([3, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    false,
                    pal.warning,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.warning,
                    c_rad,
                ))
                .on_press(Message::SelectChannel(ChannelTab::ModelLibrary)),
        );
    } else {
        for m in app.telemetry.models.iter().take(4) {
            let is_sel = app.selected_model.as_deref() == Some(m.display_alias.as_str());
            model_row = model_row.push(
                button(text(&m.display_alias).size(10))
                    .padding(Padding::from([3, 8]))
                    .style(theme::cyber_chip_button_with_radius(
                        is_sel,
                        pal.accent_primary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::SelectModel(m.display_alias.clone())),
            );
        }
    }

    let mut kind_row = Row::new().spacing(4).align_y(Alignment::Center);
    kind_row = kind_row.push(
        text("AGENT")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    for kind in AgentKind::ALL {
        kind_row = kind_row.push(
            button(text(kind.label()).size(10))
                .padding(Padding::from([3, 8]))
                .style(theme::cyber_chip_button_with_radius(
                    app.agent_kind == kind,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectAgentKind(kind)),
        );
    }

    let mut effort_row = Row::new().spacing(4).align_y(Alignment::Center);
    effort_row = effort_row.push(
        text("EFFORT")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    for eff in ReasoningEffort::ALL {
        effort_row = effort_row.push(
            button(text(eff.label()).size(10).font(Font::MONOSPACE))
                .padding(Padding::from([3, 8]))
                .style(theme::cyber_chip_button_with_radius(
                    app.reasoning_effort == eff,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectReasoningEffort(eff)),
        );
    }

    let trust_color = match trust {
        FolderAccessLevel::Trusted => pal.nominal,
        FolderAccessLevel::ReadWrite => pal.accent_secondary,
        FolderAccessLevel::ReadOnly => pal.warning,
        FolderAccessLevel::Deny => pal.critical,
    };

    let ctx_bar = container(
        row![
            text(format!(
                "CTX ~{} / {} tok ({:.0}%)",
                usage.total_tokens, usage.max_tokens, usage.pct
            ))
            .size(10)
            .font(Font::MONOSPACE)
            .color(if usage.pct > 82.0 {
                pal.warning
            } else {
                pal.accent_secondary
            }),
            container(progress_bar(0.0..=100.0, usage.pct)).width(Length::Fixed(86.0)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([3, 8]))
    .style(theme::pill_container(
        pal.bg_rail,
        pal.accent_secondary,
        pal.border_subtle,
    ));

    let row_one = row![
        access_row,
        stat_badge(
            "ENGINE",
            local_status_lbl.to_string(),
            local_status_col,
            pal.bg_rail
        ),
        model_row,
        space::horizontal(),
        button(
            text(format!("FOLDER: {}", trust.label()))
                .size(10)
                .font(Font::MONOSPACE)
        )
        .padding(Padding::from([3, 8]))
        .style(theme::cyber_chip_button_with_radius(
            trust == FolderAccessLevel::Trusted,
            trust_color,
            pal.bg_rail,
            pal.border_subtle,
            pal.text_primary,
            trust_color,
            c_rad,
        ))
        .on_press(Message::SelectChannel(ChannelTab::CodeWorkspace)),
        ctx_bar,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let row_two = row![
        kind_row,
        space::Space::new().width(8),
        effort_row,
        space::horizontal(),
        checkbox(app.read_only_assistant)
            .label("Read-Only")
            .size(13)
            .text_size(10)
            .on_toggle(Message::ToggleAssistantMode),
        checkbox(app.use_rag)
            .label("RAG")
            .size(13)
            .text_size(10)
            .on_toggle(Message::ToggleRag),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    column![row_one, row_two].spacing(6).into()
}

/// Interactive SQLite **Tasks DB, Daily Log & Completed Archive** view (`ChannelTab::WorkflowPipeline`).
pub fn view_tasks_db_channel<'a>(app: &'a LoraDesktopApp) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let rad = app.dashboard.card_radius();
    let c_rad = app.dashboard.control_radius();
    let ui_font = app.dashboard.ui_font();

    let scope_pid = if app.task_scope_all_projects {
        None
    } else {
        Some(app.active_project_id)
    };
    let all_scoped = app.workspace_db.list_tasks(scope_pid);
    let total_count = all_scoped.len().max(1);
    let q_count = all_scoped
        .iter()
        .filter(|t| matches!(t.status, TaskStatus::Todo | TaskStatus::Blocked))
        .count();
    let r_count = all_scoped
        .iter()
        .filter(|t| t.status == TaskStatus::InProgress)
        .count();
    let d_count = all_scoped
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .count();
    let daily_total = all_scoped
        .iter()
        .filter(|t| t.task_kind == TaskKind::Daily)
        .count();
    let daily_done = all_scoped
        .iter()
        .filter(|t| t.task_kind == TaskKind::Daily && t.status == TaskStatus::Done)
        .count();
    let completion_pct = (d_count as f32 / total_count as f32) * 100.0;

    let search_lower = app.task_search_input.trim().to_ascii_lowercase();
    let filtered: Vec<_> = all_scoped
        .into_iter()
        .filter(|t| match app.task_filter {
            TaskFilter::All => true,
            TaskFilter::Daily => t.task_kind == TaskKind::Daily,
            TaskFilter::Code => matches!(t.task_kind, TaskKind::Code | TaskKind::Review),
            TaskFilter::Open => matches!(t.status, TaskStatus::Todo | TaskStatus::Blocked),
            TaskFilter::InProgress => t.status == TaskStatus::InProgress,
            TaskFilter::Done => t.status == TaskStatus::Done,
        })
        .filter(|t| {
            search_lower.is_empty()
                || t.title.to_ascii_lowercase().contains(&search_lower)
                || t.notes.to_ascii_lowercase().contains(&search_lower)
                || t.assignee.to_ascii_lowercase().contains(&search_lower)
                || t.task_kind
                    .label()
                    .to_ascii_lowercase()
                    .contains(&search_lower)
        })
        .collect();

    let mut filter_row = Row::new().spacing(6).align_y(Alignment::Center);
    for f in TaskFilter::ALL {
        let active = app.task_filter == f;
        filter_row = filter_row.push(
            button(text(f.label()).size(11))
                .padding(Padding::from([4, 10]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    if f == TaskFilter::Done {
                        pal.nominal
                    } else {
                        pal.accent_primary
                    },
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectTaskFilter(f)),
        );
    }

    let summary_bar = section_card(
        pal,
        rad,
        14,
        column![
            row![
                column![
                    text("Agent Tasks DB · Daily & Completed Log (SQLite WAL)")
                        .size(15)
                        .font(ui_font)
                        .color(pal.text_primary),
                    text(format!(
                        "Active project: {} · Daily Done: {daily_done}/{daily_total} · Total Completed: {d_count}",
                        app.active_project_name()
                    ))
                    .size(11)
                    .color(pal.text_muted),
                ]
                .spacing(2)
                .width(Length::Fill),
                stat_badge("QUEUED", q_count.to_string(), pal.warning, pal.bg_elevated),
                stat_badge(
                    "RUNNING",
                    r_count.to_string(),
                    pal.accent_secondary,
                    pal.accent_secondary_soft
                ),
                stat_badge("DONE", d_count.to_string(), pal.nominal, pal.nominal_soft),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text(format!("Completion {:.0}%", completion_pct))
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(pal.nominal),
                progress_bar(0.0..=100.0, completion_pct),
                button(text("Daily Summary in Agent").size(10).font(Font::MONOSPACE))
                    .padding(Padding::from([4, 10]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.accent_secondary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.accent_secondary,
                        c_rad,
                    ))
                    .on_press(Message::SummarizeDailyTasksInAgent),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row![
                filter_row,
                button(
                    text(if app.task_scope_all_projects {
                        "Scope: All Projects"
                    } else {
                        "Scope: Active Project"
                    })
                    .size(11)
                )
                .padding(Padding::from([4, 10]))
                .style(theme::cyber_chip_button_with_radius(
                    app.task_scope_all_projects,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_secondary,
                    c_rad,
                ))
                .on_press(Message::ToggleTaskAllProjects),
                text_input("Filter tasks...", &app.task_search_input)
                    .on_input(Message::TaskSearchChanged)
                    .padding(Padding::from([5, 10]))
                    .size(12)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_secondary,
                        pal.accent_secondary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(10),
    );

    let mut prio_row = Row::new().spacing(4).align_y(Alignment::Center);
    for p in TaskPriority::ALL {
        let active = app.new_task_priority == p;
        prio_row = prio_row.push(
            button(text(p.as_str()).size(10).font(Font::MONOSPACE))
                .padding(Padding::from([4, 8]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::NewTaskPrioritySelected(p)),
        );
    }

    let mut kind_row = Row::new().spacing(4).align_y(Alignment::Center);
    for tk in TaskKind::ALL {
        let active = app.new_task_kind == tk;
        kind_row = kind_row.push(
            button(text(tk.label()).size(10).font(Font::MONOSPACE))
                .padding(Padding::from([4, 8]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::NewTaskKindSelected(tk)),
        );
    }

    let creator = section_card(
        pal,
        rad,
        14,
        column![
            row![
                text("New Agent / Daily / Code Task")
                    .size(13)
                    .font(ui_font)
                    .color(pal.text_primary),
                space::horizontal(),
                kind_row,
                space::Space::new().width(8),
                prio_row,
            ]
            .align_y(Alignment::Center),
            row![
                text_input("Task objective or prompt...", &app.new_task_title)
                    .on_input(Message::NewTaskTitleChanged)
                    .on_submit(Message::CreateAgentTask)
                    .padding(Padding::from([8, 12]))
                    .size(12)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_primary,
                        pal.accent_primary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::FillPortion(2)),
                text_input("Context / notes (optional)...", &app.new_task_notes)
                    .on_input(Message::NewTaskNotesChanged)
                    .padding(Padding::from([8, 12]))
                    .size(12)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_secondary,
                        pal.accent_secondary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::FillPortion(2)),
                text_input("Agent role...", &app.new_task_assignee)
                    .on_input(Message::NewTaskAssigneeChanged)
                    .padding(Padding::from([8, 12]))
                    .size(12)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_secondary,
                        pal.accent_secondary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::Fixed(130.0)),
                button(text("+ Add Task").size(12))
                    .padding(Padding::from([8, 14]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_primary,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::CreateAgentTask),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(8),
    );

    let mut task_rows = Column::new().spacing(8);
    if filtered.is_empty() {
        task_rows = task_rows.push(section_card(
            pal,
            rad,
            14,
            text("No matching tasks in SQLite for this filter.")
                .size(12)
                .color(pal.text_muted),
        ));
    } else {
        for task in filtered {
            let status_color = match task.status {
                TaskStatus::Todo => pal.warning,
                TaskStatus::InProgress => pal.accent_secondary,
                TaskStatus::Blocked => pal.critical,
                TaskStatus::Done => pal.nominal,
            };
            let proj_name = app
                .projects_cache
                .iter()
                .find(|p| p.id == task.project_id)
                .map(|p| p.name.as_str())
                .unwrap_or("Project");
            let prompt_payload = if task.notes.trim().is_empty() {
                task.title.clone()
            } else {
                format!("{}\n\nContext: {}", task.title, task.notes)
            };

            let is_done = task.status == TaskStatus::Done;
            let card = row![
                container(space::Space::new().width(3).height(Length::Fill))
                    .style(theme::palette_surface_container(status_color, status_color)),
                column![
                    row![
                        container(text(task.priority.as_str()).size(10).font(Font::MONOSPACE))
                            .padding(Padding::from([2, 6]))
                            .style(theme::pill_container(
                                pal.bg_elevated,
                                pal.accent_primary,
                                pal.border_subtle,
                            )),
                        container(text(task.task_kind.label()).size(10).font(Font::MONOSPACE))
                            .padding(Padding::from([2, 6]))
                            .style(theme::pill_container(
                                pal.bg_elevated,
                                pal.accent_secondary,
                                pal.border_subtle,
                            )),
                        container(text(task.status.label()).size(10).font(Font::MONOSPACE))
                            .padding(Padding::from([2, 7]))
                            .style(theme::pill_container(
                                pal.bg_rail,
                                status_color,
                                status_color,
                            )),
                        text(if task.completed_at_ms.is_some() && is_done {
                            format!("{proj_name} · {} · Completed", task.assignee)
                        } else {
                            format!("{proj_name} · {}", task.assignee)
                        })
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(if is_done {
                            pal.nominal
                        } else {
                            pal.text_muted
                        }),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    text(task.title.clone()).size(13).color(pal.text_primary),
                    if task.notes.trim().is_empty() {
                        Element::from(space::Space::new().height(0))
                    } else {
                        text(task.notes.clone())
                            .size(11)
                            .color(pal.text_muted)
                            .into()
                    },
                ]
                .spacing(4)
                .width(Length::Fill),
                button(text("Run in Agent").size(11))
                    .padding(Padding::from([6, 11]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_secondary,
                        pal.accent_primary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::ExecuteTaskInAgent {
                        task_id: task.id,
                        prompt: prompt_payload,
                    }),
                button(text(if is_done { "Reopen" } else { "Done ✓" }).size(11))
                    .padding(Padding::from([6, 10]))
                    .style(theme::cyber_chip_button_with_radius(
                        is_done,
                        pal.nominal,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.nominal,
                        c_rad,
                    ))
                    .on_press(Message::SetTaskStatus {
                        task_id: task.id,
                        status: if is_done {
                            TaskStatus::Todo
                        } else {
                            TaskStatus::Done
                        },
                    }),
                button(text("Advance").size(11))
                    .padding(Padding::from([6, 10]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        status_color,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        status_color,
                        c_rad,
                    ))
                    .on_press(Message::SetTaskStatus {
                        task_id: task.id,
                        status: task.status.next(),
                    }),
                button(text("Del").size(11))
                    .padding(Padding::from([6, 9]))
                    .style(theme::solid_button_with_radius(
                        Color {
                            a: 0.16,
                            ..pal.critical
                        },
                        pal.critical,
                        pal.text_primary,
                        c_rad,
                    ))
                    .on_press(Message::DeleteAgentTask(task.id)),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            task_rows = task_rows.push(section_card(pal, rad, 10, card));
        }
    }

    let (checked, valid) = app.telemetry.workflow_integrity;
    let mut receipt_list = Column::new().spacing(6);
    for r in app.telemetry.workflow_receipts.iter().take(5) {
        let short_sha: String = r.receipt_sha256.chars().take(18).collect();
        receipt_list = receipt_list.push(
            text(format!(
                "{} · {}/{} steps · {}ms · ok={} · sha256:{short_sha}...",
                r.workflow_id, r.completed_steps, r.total_steps, r.total_duration_ms, r.successful
            ))
            .size(11)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
        );
    }

    let workflow_footer = section_card(
        pal,
        rad,
        14,
        column![
            row![
                column![
                    text("Deterministic Workflow Verification Receipts")
                        .size(13)
                        .color(pal.text_primary),
                    text(format!(
                        "Chain integrity: {valid}/{checked} verified (intact={})",
                        checked == valid
                    ))
                    .size(11)
                    .color(if checked == valid {
                        pal.nominal
                    } else {
                        pal.critical
                    }),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text("Execute Verification Pipeline").size(11))
                    .padding(Padding::from([7, 14]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.nominal,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.nominal,
                        c_rad,
                    ))
                    .on_press(Message::RunWorkflowPressed),
            ]
            .align_y(Alignment::Center),
            receipt_list,
        ]
        .spacing(8),
    );

    scrollable(
        column![summary_bar, creator, task_rows, workflow_footer]
            .spacing(12)
            .padding(18),
    )
    .height(Length::Fill)
    .into()
}

/// Interactive **Code Workspace, Trusted Folders & `.env` Secrets Studio** (`ChannelTab::CodeWorkspace`).
pub fn view_code_env_channel<'a>(app: &'a LoraDesktopApp) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let rad = app.dashboard.card_radius();
    let c_rad = app.dashboard.control_radius();
    let ui_font = app.dashboard.ui_font();

    let hud_card = section_card(
        pal,
        rad,
        14,
        column![
            row![
                text("Code Execution HUD · Model, Effort, Access & Context")
                    .size(15)
                    .font(ui_font)
                    .color(pal.text_primary)
                    .width(Length::Fill),
                button(text("Open #chat ->").size(11))
                    .padding(Padding::from([5, 11]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_primary,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::SelectChannel(ChannelTab::Chat)),
            ]
            .align_y(Alignment::Center),
            view_agent_execution_bar(app),
            row![
                text_input(
                    "Attach file path (or drop files / folders / .env anywhere)...",
                    &app.attach_path_input,
                )
                .on_input(Message::AttachPathChanged)
                .on_submit(Message::AttachPathSubmit)
                .padding(Padding::from([7, 11]))
                .size(12)
                .font(Font::MONOSPACE)
                .style(theme::palette_input_style(
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.accent_secondary,
                    pal.accent_secondary_soft,
                    pal.text_primary,
                    pal.text_muted,
                    pal.text_muted,
                    c_rad,
                ))
                .width(Length::Fill),
                button(text("+ Stage File / Folder").size(11))
                    .padding(Padding::from([7, 13]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.accent_secondary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.accent_secondary,
                        c_rad,
                    ))
                    .on_press(Message::AttachPathSubmit),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(10),
    );

    // Quick Workspace Explorer from active root
    let mut explorer_rows = Column::new().spacing(5);
    let root_path = std::path::Path::new(app.workspace_root_input.trim());
    let mut listed = 0usize;
    if let Ok(rd) = std::fs::read_dir(root_path) {
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| (!e.path().is_dir(), e.file_name()));
        for ent in entries {
            let fname = ent.file_name().to_string_lossy().to_string();
            if matches!(fname.as_str(), ".git" | "target" | "node_modules") {
                continue;
            }
            let path = ent.path();
            let is_dir = path.is_dir();
            let trust = app
                .workspace_db
                .folder_trust_for_path(app.active_project_id, &path.to_string_lossy());
            let action_lbl = if is_dir { "Trust Dir" } else { "Attach" };
            let full_str = path.to_string_lossy().to_string();
            explorer_rows = explorer_rows.push(
                row![
                    container(
                        text(if is_dir { "DIR" } else { "FILE" })
                            .size(9)
                            .font(Font::MONOSPACE)
                    )
                    .padding(Padding::from([2, 6]))
                    .style(theme::pill_container(
                        pal.bg_rail,
                        if is_dir {
                            pal.accent_secondary
                        } else {
                            pal.accent_primary
                        },
                        pal.border_subtle,
                    )),
                    text(fname)
                        .size(12)
                        .font(Font::MONOSPACE)
                        .color(pal.text_primary)
                        .width(Length::Fill),
                    text(trust.label())
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(pal.text_muted),
                    button(text(action_lbl).size(10))
                        .padding(Padding::from([3, 9]))
                        .style(theme::cyber_chip_button_with_radius(
                            false,
                            pal.accent_secondary,
                            pal.bg_elevated,
                            pal.border_subtle,
                            pal.text_primary,
                            pal.accent_secondary,
                            c_rad,
                        ))
                        .on_press(Message::QuickAttachOrTrustPath(full_str)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
            listed += 1;
            if listed >= 14 {
                break;
            }
        }
    }
    if listed == 0 {
        explorer_rows = explorer_rows.push(
            text("Workspace root unreadable or empty. Adjust path in Projects or Trusted Folders.")
                .size(11)
                .color(pal.text_muted),
        );
    }

    let explorer_card = section_card(
        pal,
        rad,
        14,
        column![
            row![
                text("Workspace Code Explorer")
                    .size(14)
                    .font(ui_font)
                    .color(pal.text_primary)
                    .width(Length::Fill),
                text(app.workspace_root_input.as_str())
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(pal.accent_secondary),
            ]
            .align_y(Alignment::Center),
            explorer_rows,
        ]
        .spacing(8),
    );

    // Trusted Folders & Access Control
    let mut level_row = Row::new().spacing(4).align_y(Alignment::Center);
    for lvl in FolderAccessLevel::ALL {
        let active = app.new_trust_level == lvl;
        level_row = level_row.push(
            button(text(lvl.label()).size(10).font(Font::MONOSPACE))
                .padding(Padding::from([4, 8]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.nominal,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::NewTrustLevelSelected(lvl)),
        );
    }

    let mut folder_rows = Column::new().spacing(6);
    if app.trusted_folders.is_empty() {
        folder_rows = folder_rows.push(
            text("No folder rules configured for active project.")
                .size(11)
                .color(pal.text_muted),
        );
    } else {
        for f in &app.trusted_folders {
            let lvl_color = match f.access_level {
                FolderAccessLevel::Trusted => pal.nominal,
                FolderAccessLevel::ReadWrite => pal.accent_secondary,
                FolderAccessLevel::ReadOnly => pal.warning,
                FolderAccessLevel::Deny => pal.critical,
            };
            folder_rows = folder_rows.push(
                container(
                    row![
                        text(&f.path)
                            .size(11)
                            .font(Font::MONOSPACE)
                            .color(pal.text_primary)
                            .width(Length::Fill),
                        button(text(f.access_level.label()).size(10).font(Font::MONOSPACE))
                            .padding(Padding::from([3, 9]))
                            .style(theme::cyber_chip_button_with_radius(
                                true,
                                lvl_color,
                                pal.bg_rail,
                                lvl_color,
                                pal.text_primary,
                                lvl_color,
                                c_rad,
                            ))
                            .on_press(Message::CycleTrustedFolderLevel {
                                folder_id: f.id,
                                next_level: f.access_level.next(),
                            }),
                        button(text("Del").size(10))
                            .padding(Padding::from([3, 8]))
                            .style(theme::solid_button_with_radius(
                                Color {
                                    a: 0.16,
                                    ..pal.critical
                                },
                                pal.critical,
                                pal.text_primary,
                                c_rad,
                            ))
                            .on_press(Message::DeleteTrustedFolder(f.id)),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .padding(8)
                .width(Length::Fill)
                .style(theme::palette_card_container(
                    pal.bg_elevated,
                    pal.text_primary,
                    pal.border_subtle,
                    c_rad,
                )),
            );
        }
    }

    let trust_card = section_card(
        pal,
        rad,
        14,
        column![
            row![
                text("Folder Access & Trusted Folders Governance")
                    .size(14)
                    .font(ui_font)
                    .color(pal.text_primary)
                    .width(Length::Fill),
                button(text("Trust Active Root").size(10).font(Font::MONOSPACE))
                    .padding(Padding::from([4, 10]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.nominal,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.nominal,
                        c_rad,
                    ))
                    .on_press(Message::TrustActiveWorkspaceRoot),
            ]
            .align_y(Alignment::Center),
            level_row,
            row![
                text_input("Folder path to grant/trust...", &app.new_trust_path)
                    .on_input(Message::NewTrustPathChanged)
                    .on_submit(Message::AddTrustedFolder)
                    .padding(Padding::from([7, 11]))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.nominal,
                        pal.nominal_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::Fill),
                button(text("+ Add Folder Rule").size(11))
                    .padding(Padding::from([7, 13]))
                    .style(theme::solid_button_with_radius(
                        pal.nominal,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::AddTrustedFolder),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            folder_rows,
        ]
        .spacing(9),
    );

    // .env & Scoped Environment Variables + API Endpoint Card
    let mut env_rows = Column::new().spacing(6);
    if app.env_vars.is_empty() {
        env_rows = env_rows.push(
            text("No scoped environment variables yet. Add keys or drop a `.env` file onto the window.")
                .size(11)
                .color(pal.text_muted),
        );
    } else {
        for ev in &app.env_vars {
            let revealed = app.reveal_secret_ids.contains(&ev.id);
            let display_val = if ev.is_secret && !revealed {
                "••••••••••••".to_string()
            } else {
                ev.env_val.clone()
            };
            let scope_tag = if ev.project_id == 0 { "GLOBAL" } else { "PROJ" };
            let mut r = row![
                container(text(scope_tag).size(9).font(Font::MONOSPACE))
                    .padding(Padding::from([2, 6]))
                    .style(theme::pill_container(
                        pal.bg_rail,
                        if ev.project_id == 0 {
                            pal.accent_secondary
                        } else {
                            pal.accent_primary
                        },
                        pal.border_subtle,
                    )),
                text(&ev.env_key)
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(pal.accent_secondary),
                text("=")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(pal.text_muted),
                text(display_val)
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(if ev.enabled {
                        pal.text_primary
                    } else {
                        pal.text_muted
                    })
                    .width(Length::Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center);

            if ev.is_secret {
                r = r.push(
                    button(text(if revealed { "Hide" } else { "Show" }).size(10))
                        .padding(Padding::from([3, 8]))
                        .style(theme::cyber_chip_button_with_radius(
                            revealed,
                            pal.warning,
                            pal.bg_rail,
                            pal.border_subtle,
                            pal.text_primary,
                            pal.warning,
                            c_rad,
                        ))
                        .on_press(Message::ToggleRevealSecret(ev.id)),
                );
            }
            r = r
                .push(
                    button(text(if ev.enabled { "Active" } else { "Off" }).size(10))
                        .padding(Padding::from([3, 8]))
                        .style(theme::cyber_chip_button_with_radius(
                            ev.enabled,
                            pal.nominal,
                            pal.bg_rail,
                            pal.border_subtle,
                            pal.text_primary,
                            pal.text_muted,
                            c_rad,
                        ))
                        .on_press(Message::ToggleEnvVarEnabled {
                            env_id: ev.id,
                            enabled: !ev.enabled,
                        }),
                )
                .push(
                    button(text("Del").size(10))
                        .padding(Padding::from([3, 8]))
                        .style(theme::solid_button_with_radius(
                            Color {
                                a: 0.16,
                                ..pal.critical
                            },
                            pal.critical,
                            pal.text_primary,
                            c_rad,
                        ))
                        .on_press(Message::DeleteEnvVar(ev.id)),
                );

            env_rows = env_rows.push(container(r).padding(7).width(Length::Fill).style(
                theme::palette_card_container(
                    pal.bg_elevated,
                    pal.text_primary,
                    pal.border_subtle,
                    c_rad,
                ),
            ));
        }
    }

    let api_key_found = app
        .workspace_db
        .resolve_env_value(app.active_project_id, &app.api_config.api_key_env)
        .is_some();

    let env_card = section_card(
        pal,
        rad,
        14,
        column![
            row![
                text("Environment Variables (.env) & API Access Gateway")
                    .size(14)
                    .font(ui_font)
                    .color(pal.text_primary)
                    .width(Length::Fill),
                button(
                    text(if app.env_scope_global {
                        "Target: Global Env"
                    } else {
                        "Target: Active Project"
                    })
                    .size(10)
                    .font(Font::MONOSPACE)
                )
                .padding(Padding::from([4, 10]))
                .style(theme::cyber_chip_button_with_radius(
                    app.env_scope_global,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_secondary,
                    c_rad,
                ))
                .on_press(Message::ToggleEnvScopeGlobal),
                button(text("Import Workspace .env").size(10).font(Font::MONOSPACE))
                    .padding(Padding::from([4, 10]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.accent_primary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.accent_primary,
                        c_rad,
                    ))
                    .on_press(Message::ImportWorkspaceDotenv),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text_input("KEY (e.g. OPENAI_API_KEY)...", &app.new_env_key)
                    .on_input(Message::NewEnvKeyChanged)
                    .padding(Padding::from([7, 10]))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_secondary,
                        pal.accent_secondary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::FillPortion(2)),
                text_input("VALUE...", &app.new_env_val)
                    .on_input(Message::NewEnvValChanged)
                    .on_submit(Message::SaveEnvVar)
                    .padding(Padding::from([7, 10]))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_primary,
                        pal.accent_primary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::FillPortion(3)),
                checkbox(app.new_env_secret)
                    .label("Secret")
                    .size(14)
                    .text_size(11)
                    .on_toggle(Message::NewEnvSecretToggled),
                button(text("+ Save Env").size(11))
                    .padding(Padding::from([7, 13]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_primary,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::SaveEnvVar),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            env_rows,
            rule::horizontal(1).style(theme::palette_divider_rule(pal)),
            row![
                text("API ROUTE CONFIG")
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(pal.warning),
                container(
                    text(if api_key_found {
                        "CREDENTIAL RESOLVED"
                    } else {
                        "KEY MISSING IN .ENV"
                    })
                    .size(9)
                    .font(Font::MONOSPACE)
                )
                .padding(Padding::from([2, 7]))
                .style(theme::pill_container(
                    pal.bg_rail,
                    if api_key_found {
                        pal.nominal
                    } else {
                        pal.critical
                    },
                    if api_key_found {
                        pal.nominal
                    } else {
                        pal.critical
                    },
                )),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text_input(
                    "Base URL (https://api.openai.com/v1)...",
                    &app.api_config.base_url
                )
                .on_input(Message::ApiBaseUrlChanged)
                .padding(Padding::from([6, 10]))
                .size(11)
                .font(Font::MONOSPACE)
                .style(theme::palette_input_style(
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.warning,
                    pal.bg_rail,
                    pal.text_primary,
                    pal.text_muted,
                    pal.text_muted,
                    c_rad,
                ))
                .width(Length::FillPortion(2)),
                text_input("API Model...", &app.api_config.model_name)
                    .on_input(Message::ApiModelNameChanged)
                    .padding(Padding::from([6, 10]))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.warning,
                        pal.bg_rail,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::FillPortion(1)),
                text_input("Key Env Name...", &app.api_config.api_key_env)
                    .on_input(Message::ApiKeyEnvChanged)
                    .padding(Padding::from([6, 10]))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.warning,
                        pal.bg_rail,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::Fixed(160.0)),
            ]
            .spacing(8),
        ]
        .spacing(9),
    );

    scrollable(
        column![hud_card, explorer_card, trust_card, env_card]
            .spacing(12)
            .padding(18),
    )
    .height(Length::Fill)
    .into()
}

/// Interactive **Project & Section Builder** + Scoped Governance view (`ChannelTab::AgentWorkspace`).
pub fn view_projects_channel<'a>(app: &'a LoraDesktopApp) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let rad = app.dashboard.card_radius();
    let c_rad = app.dashboard.control_radius();
    let ui_font = app.dashboard.ui_font();

    let creator = section_card(
        pal,
        rad,
        16,
        column![
            text("Create New Workspace Project / Section")
                .size(15)
                .font(ui_font)
                .color(pal.text_primary),
            text("Each project isolates its active root directory, agent instructions, and Tasks DB view.")
                .size(11)
                .color(pal.text_muted),
            row![
                text_input("Project name (e.g. Lora Inference Lab)...", &app.new_project_name)
                    .on_input(Message::NewProjectNameChanged)
                    .padding(Padding::from([8, 12]))
                    .size(12)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_primary,
                        pal.accent_primary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::FillPortion(2)),
                text_input("Badge tag (e.g. LAB)...", &app.new_project_tag)
                    .on_input(Message::NewProjectTagChanged)
                    .padding(Padding::from([8, 12]))
                    .size(12)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_secondary,
                        pal.accent_secondary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::Fixed(130.0)),
            ]
            .spacing(8),
            text_input("Scoped workspace directory path...", &app.new_project_root)
                .on_input(Message::NewProjectRootChanged)
                .padding(Padding::from([8, 12]))
                .size(12)
                .font(Font::MONOSPACE)
                .style(theme::palette_input_style(
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.accent_secondary,
                    pal.accent_secondary_soft,
                    pal.text_primary,
                    pal.text_muted,
                    pal.text_muted,
                    c_rad,
                ))
                .width(Length::Fill),
            row![
                text_input("Short project summary...", &app.new_project_desc)
                    .on_input(Message::NewProjectDescChanged)
                    .padding(Padding::from([8, 12]))
                    .size(12)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_secondary,
                        pal.accent_secondary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::Fill),
                text_input("Agent system rule...", &app.new_project_prompt)
                    .on_input(Message::NewProjectPromptChanged)
                    .padding(Padding::from([8, 12]))
                    .size(12)
                    .style(theme::palette_input_style(
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.accent_primary,
                        pal.accent_primary_soft,
                        pal.text_primary,
                        pal.text_muted,
                        pal.text_muted,
                        c_rad,
                    ))
                    .width(Length::Fill),
                button(text("+ Create Project").size(12))
                    .padding(Padding::from([8, 16]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_primary,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::CreateWorkspaceProject),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(8),
    );

    let mut project_list = Column::new().spacing(8);
    let can_delete = app.projects_cache.len() > 1;
    for proj in &app.projects_cache {
        let active = proj.id == app.active_project_id;
        let tasks_count = app.workspace_db.list_tasks(Some(proj.id)).len();

        let mut actions = Row::new().spacing(8).align_y(Alignment::Center);
        actions = actions.push(
            button(text(if active { "Active" } else { "Activate" }).size(11))
                .padding(Padding::from([6, 12]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_secondary,
                    c_rad,
                ))
                .on_press(Message::SelectWorkspaceProject(proj.id)),
        );
        if can_delete {
            actions = actions.push(
                button(text("Remove").size(11))
                    .padding(Padding::from([6, 10]))
                    .style(theme::solid_button_with_radius(
                        Color {
                            a: 0.16,
                            ..pal.critical
                        },
                        pal.critical,
                        pal.text_primary,
                        c_rad,
                    ))
                    .on_press(Message::DeleteWorkspaceProject(proj.id)),
            );
        }

        let card_content = row![
            column![
                row![
                    container(text(&proj.accent_tag).size(10).font(Font::MONOSPACE))
                        .padding(Padding::from([2, 7]))
                        .style(theme::pill_container(
                            if active {
                                pal.accent_primary_soft
                            } else {
                                pal.bg_elevated
                            },
                            if active {
                                pal.accent_primary
                            } else {
                                pal.accent_secondary
                            },
                            if active {
                                pal.accent_primary
                            } else {
                                pal.border_subtle
                            },
                        )),
                    text(&proj.name).size(14).color(pal.text_primary),
                    text(format!("{tasks_count} tasks"))
                        .size(11)
                        .font(Font::MONOSPACE)
                        .color(pal.text_muted),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                text(&proj.root_path)
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(pal.accent_secondary),
                text(&proj.description).size(11).color(pal.text_secondary),
            ]
            .spacing(4)
            .width(Length::Fill),
            actions,
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        project_list = project_list.push(
            container(card_content)
                .padding(14)
                .width(Length::Fill)
                .style(theme::palette_card_container(
                    pal.bg_card,
                    pal.text_primary,
                    if active {
                        pal.accent_primary
                    } else {
                        pal.border_subtle
                    },
                    rad,
                )),
        );
    }

    let items: Vec<String> = (0..120)
        .map(|i| format!("transcript-entry-{i:04}"))
        .collect();
    let slice = slice_virtualized_transcript(&items, app.transcript_offset, 10);

    let scope_security = section_card(
        pal,
        rad,
        14,
        column![
            row![
                text("Active Scope & Trusted Consent Sandbox")
                    .size(13)
                    .color(pal.text_primary)
                    .width(Length::Fill),
                button(text("Preview Default-Deny Modal").size(11))
                    .padding(Padding::from([6, 12]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.warning,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.warning,
                        c_rad,
                    ))
                    .on_press(Message::PreviewConsentDialogPressed),
            ]
            .align_y(Alignment::Center),
            text_input("Workspace root directory...", &app.workspace_root_input)
                .on_input(Message::WorkspacePathChanged)
                .padding(Padding::from([7, 11]))
                .size(12)
                .font(Font::MONOSPACE)
                .style(theme::palette_input_style(
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.accent_secondary,
                    pal.accent_secondary_soft,
                    pal.text_primary,
                    pal.text_muted,
                    pal.text_muted,
                    c_rad,
                ))
                .width(Length::Fill),
            row![
                text(format!(
                    "Virtualized Transcript Window (R12): {}..{} of {}",
                    slice.start_index, slice.end_index, slice.total_items
                ))
                .size(11)
                .color(pal.text_muted)
                .width(Length::Fill),
                button(text("Prev").size(10))
                    .padding(Padding::from([3, 9]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.accent_primary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::SliceTranscriptStep(
                        app.transcript_offset.saturating_sub(10)
                    )),
                button(text("Next").size(10))
                    .padding(Padding::from([3, 9]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.accent_primary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::SliceTranscriptStep(
                        (app.transcript_offset + 10).min(110)
                    )),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ]
        .spacing(8),
    );

    scrollable(
        column![creator, project_list, scope_security]
            .spacing(12)
            .padding(18),
    )
    .height(Length::Fill)
    .into()
}

/// Full **`llama.cpp` Runtime Engine Control Panel** & Hardware HIE view (`ChannelTab::Hardware`).
pub fn view_llamacpp_channel<'a>(app: &'a LoraDesktopApp) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let rad = app.dashboard.card_radius();
    let c_rad = app.dashboard.control_radius();
    let ui_font = app.dashboard.ui_font();
    let cfg = &app.llama_config;

    let host_card = if let Some(h) = &app.telemetry.host {
        let total_mib = (h.total_ram_bytes / (1024 * 1024)).max(1) as f32;
        let avail_mib = (h.available_ram_bytes / (1024 * 1024)) as f32;
        let used_pct = ((total_mib - avail_mib) / total_mib * 100.0).clamp(0.0, 100.0);

        section_card(
            pal,
            rad,
            16,
            column![
                row![
                    text(format!("{} · {} ({})", h.cpu_brand, h.os_name, h.cpu_arch))
                        .size(14)
                        .font(ui_font)
                        .color(pal.text_primary)
                        .width(Length::Fill),
                    stat_badge(
                        "EST KV",
                        format!("{:.1} MiB", cfg.estimated_kv_mib()),
                        pal.accent_secondary,
                        pal.accent_secondary_soft
                    ),
                ]
                .align_y(Alignment::Center),
                text(format!(
                    "{} physical / {} logical cores · Free RAM: {:.0} MiB / {:.0} MiB · Free Disk: {} GiB · Power: {:?}",
                    h.physical_cores,
                    h.logical_cores,
                    avail_mib,
                    total_mib,
                    h.free_disk_bytes / (1024 * 1024 * 1024),
                    h.charging_state
                ))
                .size(12)
                .color(pal.text_muted),
                progress_bar(0.0..=100.0, used_pct),
                text(format!(
                    "CPU ISA Extensions: {}",
                    if h.cpu_isa_features.is_empty() {
                        "baseline".to_string()
                    } else {
                        h.cpu_isa_features.join(", ")
                    }
                ))
                .size(11)
                .font(Font::MONOSPACE)
                .color(pal.text_muted),
            ]
            .spacing(8),
        )
    } else {
        section_card(
            pal,
            rad,
            16,
            text("Hardware telemetry probe unavailable.")
                .size(12)
                .color(pal.text_muted),
        )
    };

    let mut batch_row = Row::new().spacing(6).align_y(Alignment::Center);
    batch_row = batch_row.push(
        text("BATCH")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    for b in [128_u32, 256, 512, 1024, 2048] {
        let active = cfg.n_batch == b;
        batch_row = batch_row.push(
            button(text(b.to_string()).size(11).font(Font::MONOSPACE))
                .padding(Padding::from([4, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::LlamaBatchChanged(b)),
        );
    }

    let mut kv_row = Row::new().spacing(6).align_y(Alignment::Center);
    kv_row = kv_row.push(
        text("KV QUANT")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    for kv in KvCacheType::ALL {
        let active = cfg.kv_cache_type == kv;
        kv_row = kv_row.push(
            button(text(kv.label()).size(11))
                .padding(Padding::from([4, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::LlamaKvQuantChanged(kv)),
        );
    }

    let kernel_card = section_card(
        pal,
        rad,
        16,
        column![
            row![
                text("llama.cpp GPU Offload & Execution Kernel")
                    .size(14)
                    .font(ui_font)
                    .color(pal.text_primary)
                    .width(Length::Fill),
                button(text("Reset Engine Defaults").size(10))
                    .padding(Padding::from([4, 9]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.warning,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.warning,
                        c_rad,
                    ))
                    .on_press(Message::ResetLlamaConfig),
            ]
            .align_y(Alignment::Center),
            row![
                text(format!("GPU Layers (-ngl): {}", cfg.n_gpu_layers))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.accent_primary)
                    .width(Length::Fixed(185.0)),
                slider(0..=99, cfg.n_gpu_layers, Message::LlamaGpuLayersChanged)
                    .step(1_u32)
                    .width(Length::Fill),
                button(text("CPU").size(10))
                    .padding(Padding::from([3, 7]))
                    .style(theme::cyber_chip_button_with_radius(
                        cfg.n_gpu_layers == 0,
                        pal.accent_secondary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_muted,
                        c_rad,
                    ))
                    .on_press(Message::LlamaGpuLayersChanged(0)),
                button(text("MAX").size(10))
                    .padding(Padding::from([3, 7]))
                    .style(theme::cyber_chip_button_with_radius(
                        cfg.n_gpu_layers == 99,
                        pal.accent_primary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_muted,
                        c_rad,
                    ))
                    .on_press(Message::LlamaGpuLayersChanged(99)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text(format!("CPU Threads (-t): {}", cfg.n_threads))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.accent_secondary)
                    .width(Length::Fixed(185.0)),
                slider(1..=64, cfg.n_threads, Message::LlamaThreadsChanged)
                    .step(1_u32)
                    .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![batch_row, space::horizontal(), kv_row]
                .spacing(10)
                .align_y(Alignment::Center),
            row![
                checkbox(cfg.flash_attention)
                    .label("Flash Attention (--flash-attn)")
                    .size(14)
                    .text_size(12)
                    .on_toggle(Message::LlamaToggleFlashAttention),
                checkbox(cfg.kv_offload)
                    .label("KV Offload to GPU")
                    .size(14)
                    .text_size(12)
                    .on_toggle(Message::LlamaToggleKvOffload),
                checkbox(cfg.mmap)
                    .label("mmap")
                    .size(14)
                    .text_size(12)
                    .on_toggle(Message::LlamaToggleMmap),
                checkbox(cfg.mlock)
                    .label("mlock RAM")
                    .size(14)
                    .text_size(12)
                    .on_toggle(Message::LlamaToggleMlock),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        ]
        .spacing(10),
    );

    let profiles = [
        TuningProfile::Automatic,
        TuningProfile::Balanced,
        TuningProfile::MaximumPerformance,
        TuningProfile::LowMemory,
        TuningProfile::BatterySaver,
    ];
    let mut prof_row = Row::new().spacing(6);
    for p in profiles {
        let active = app.tuning_profile == p;
        prof_row = prof_row.push(
            button(text(p.as_str()).size(11))
                .padding(Padding::from([5, 10]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_secondary,
                    c_rad,
                ))
                .on_press(Message::SelectTuningProfile(p)),
        );
    }

    let mut ctx_row = Row::new().spacing(6);
    for ctx in [1024_u32, 2048, 4096, 8192, 16384] {
        let active = app.requested_context == ctx;
        ctx_row = ctx_row.push(
            button(text(format!("{ctx} tok")).size(11).font(Font::MONOSPACE))
                .padding(Padding::from([5, 10]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_secondary,
                    c_rad,
                ))
                .on_press(Message::ContextTokensChanged(ctx)),
        );
    }

    let budget_summary = match allocate_context_budget(&ContextBudgetRequest {
        context_window_tokens: app.requested_context,
        reserved_output_tokens: 512,
        control_schema_overhead_tokens: 128,
        user_request_tokens: 256,
        recent_turns_tokens: 128,
        tool_receipts_tokens: 64,
        ranked_excerpts: vec![],
    }) {
        Ok(alloc) => format!(
            "Context Budget Ready: window={} · allocated={} · headroom={} · reserved_out={}",
            alloc.context_window_tokens,
            alloc.total_allocated_tokens,
            alloc.remaining_headroom_tokens,
            alloc.reserved_output_tokens
        ),
        Err(e) => format!("Budget Error: {e}"),
    };

    let sampling_card = section_card(
        pal,
        rad,
        16,
        column![
            text("Sampling Matrix & Context Window Allocation")
                .size(14)
                .font(ui_font)
                .color(pal.text_primary),
            prof_row,
            ctx_row,
            row![
                text(format!(
                    "Temperature: {:.2}",
                    cfg.temperature_x100 as f32 / 100.0
                ))
                .size(11)
                .font(Font::MONOSPACE)
                .color(pal.text_secondary)
                .width(Length::Fixed(165.0)),
                slider(0..=200, cfg.temperature_x100, Message::LlamaTempChanged)
                    .step(5_u32)
                    .width(Length::FillPortion(1)),
                text(format!("Top-P: {:.2}", cfg.top_p_x100 as f32 / 100.0))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(120.0)),
                slider(10..=100, cfg.top_p_x100, Message::LlamaTopPChanged)
                    .step(5_u32)
                    .width(Length::FillPortion(1)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row![
                text(format!("Top-K: {}", cfg.top_k))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(165.0)),
                slider(1..=100, cfg.top_k, Message::LlamaTopKChanged)
                    .step(1_u32)
                    .width(Length::FillPortion(1)),
                text(format!(
                    "Repeat Pen: {:.2}",
                    cfg.repeat_penalty_x100 as f32 / 100.0
                ))
                .size(11)
                .font(Font::MONOSPACE)
                .color(pal.text_secondary)
                .width(Length::Fixed(120.0)),
                slider(
                    100..=150,
                    cfg.repeat_penalty_x100,
                    Message::LlamaRepeatPenaltyChanged
                )
                .step(5_u32)
                .width(Length::FillPortion(1)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            text(budget_summary)
                .size(11)
                .font(Font::MONOSPACE)
                .color(pal.nominal),
        ]
        .spacing(10),
    );

    let cmd_preview = container(
        column![
            text("LIVE LLAMA.CPP WORKER COMMAND PROFILE")
                .size(10)
                .font(Font::MONOSPACE)
                .color(pal.accent_secondary),
            text(cfg.cli_preview(app.selected_model.as_deref()))
                .size(11)
                .font(Font::MONOSPACE)
                .color(pal.text_primary),
        ]
        .spacing(6),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::palette_card_container(
        pal.bg_rail,
        pal.text_primary,
        pal.border_accent,
        rad,
    ));

    scrollable(
        column![host_card, kernel_card, sampling_card, cmd_preview]
            .spacing(12)
            .padding(18),
    )
    .height(Length::Fill)
    .into()
}

/// **Infinite Appearance & Advanced Settings Studio** (`ChannelTab::Settings`).
pub fn view_studio_settings_channel<'a>(app: &'a LoraDesktopApp) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let rad = app.dashboard.card_radius();
    let c_rad = app.dashboard.control_radius();
    let prefs = &app.dashboard.prefs;
    let custom = &prefs.custom_theme;

    let mut depth_row = Row::new().spacing(6).align_y(Alignment::Center);
    for depth in SurfaceDepth::ALL {
        let active = custom.surface_depth == depth;
        depth_row = depth_row.push(
            button(text(depth.label()).size(11))
                .padding(Padding::from([5, 10]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectSurfaceDepth(depth)),
        );
    }

    let custom_matrix_card = section_card(
        pal,
        rad,
        16,
        column![
            row![
                text("Infinite Cyberpunk Color & Void Generator")
                    .size(15)
                    .color(pal.text_primary)
                    .width(Length::Fill),
                container(
                    text(format!(
                        "{} · {:.1}:1",
                        pal.wcag_grade_label(),
                        pal.wcag_primary_ratio()
                    ))
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(pal.nominal),
                )
                .padding(Padding::from([3, 8]))
                .style(theme::pill_container(
                    pal.nominal_soft,
                    pal.nominal,
                    pal.nominal,
                )),
                checkbox(custom.enabled)
                    .label("Enable Custom Hue Matrix")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::ToggleCustomTheme),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row![
                container(space::Space::new().width(14).height(14)).style(theme::pill_container(
                    pal.accent_primary,
                    pal.accent_primary,
                    pal.text_primary
                )),
                text(format!("Primary Neon Hue: {}°", custom.primary_hue))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(185.0)),
                slider(
                    0..=359,
                    custom.primary_hue,
                    Message::CustomPrimaryHueChanged
                )
                .step(1_u16)
                .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                container(space::Space::new().width(14).height(14)).style(theme::pill_container(
                    pal.accent_secondary,
                    pal.accent_secondary,
                    pal.text_primary
                )),
                text(format!("Secondary Accent Hue: {}°", custom.secondary_hue))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(185.0)),
                slider(
                    0..=359,
                    custom.secondary_hue,
                    Message::CustomSecondaryHueChanged
                )
                .step(1_u16)
                .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text("SURFACE VOID")
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(pal.text_muted),
                depth_row,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(10),
    );

    let mut theme_rows = Column::new().spacing(6);
    for chunk in CyberThemePreset::ALL.chunks(4) {
        let mut r = Row::new().spacing(6);
        for &t in chunk {
            let active = !custom.enabled && prefs.theme_preset == t;
            let tp = t.palette();
            r = r.push(
                button(
                    row![
                        container(space::Space::new().width(8).height(8)).style(
                            theme::pill_container(
                                tp.accent_primary,
                                tp.accent_primary,
                                tp.accent_secondary
                            )
                        ),
                        column![
                            text(t.label()).size(11).color(if active {
                                pal.text_primary
                            } else {
                                pal.text_secondary
                            }),
                            text(t.origin())
                                .size(9)
                                .font(Font::MONOSPACE)
                                .color(pal.text_muted),
                        ]
                        .spacing(1),
                    ]
                    .spacing(7)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([6, 9]))
                .width(Length::FillPortion(1))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    tp.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectCyberTheme(t)),
            );
        }
        theme_rows = theme_rows.push(r);
    }

    let curated_gallery = section_card(
        pal,
        rad,
        16,
        column![
            text("Curated Cinema / Game / Anime Theme Presets (11 Realities)")
                .size(14)
                .color(pal.text_primary),
            theme_rows,
        ]
        .spacing(10),
    );

    let shape_chrome = section_card(
        pal,
        rad,
        16,
        column![
            text("Geometry, Chrome Layout & Micro-Customization")
                .size(14)
                .color(pal.text_primary),
            row![
                text(format!("Corner Radius: {} px", prefs.corner_radius_px))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(185.0)),
                slider(
                    0_u8..=16,
                    prefs.corner_radius_px,
                    Message::CornerRadiusChanged
                )
                .step(1_u8)
                .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text(format!("Neon Glow Power: {}%", prefs.glow_strength_pct))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(185.0)),
                slider(
                    0_u8..=100,
                    prefs.glow_strength_pct,
                    Message::GlowStrengthChanged
                )
                .step(5_u8)
                .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text(format!("Sidebar Width: {} px", prefs.sidebar_width_px))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(185.0)),
                slider(
                    210_u16..=300,
                    prefs.sidebar_width_px,
                    Message::SidebarWidthChanged
                )
                .step(4_u16)
                .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                checkbox(prefs.show_icon_rail)
                    .label("Show Far-Left Vector Rail")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::ToggleIconRail),
                checkbox(prefs.show_status_footer)
                    .label("Show Bottom Status Bar")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::ToggleStatusFooter),
                checkbox(prefs.use_mono_chrome)
                    .label("Monospace Terminal Chrome")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::ToggleMonoChrome),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
            row![
                checkbox(prefs.cyber_motion)
                    .label("Smooth 60 FPS Spring Motion & Scanlines (uncheck = Reduced Motion)")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::ToggleCyberMotion),
                checkbox(prefs.native_notifications)
                    .label("Native OS Completion Toast")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::ToggleNativeNotifications),
                checkbox(prefs.show_mini_bot)
                    .label("Animated Lora Mini-Bot Companion")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::ToggleMiniBot),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        ]
        .spacing(10),
    );

    let capri = &prefs.capri;
    let mut aura_row = Row::new().spacing(6).align_y(Alignment::Center);
    for preset in CapriAuraPreset::ALL {
        aura_row = aura_row.push(
            button(text(preset.label()).size(10))
                .padding(Padding::from([4, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    capri.aura_preset == preset,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::CapriAuraPresetChanged(preset)),
        );
    }

    let mut eye_row = Row::new().spacing(6).align_y(Alignment::Center);
    for em in CapriEyeMode::ALL {
        eye_row = eye_row.push(
            button(text(em.label()).size(10).font(Font::MONOSPACE))
                .padding(Padding::from([4, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    capri.eye_mode == em,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::CapriEyeModeChanged(em)),
        );
    }

    let capri_studio_card = section_card(
        pal,
        rad,
        16,
        column![
            row![
                view_mini_bot_avatar(app, (capri.widget_scale_px as f32).clamp(48.0, 96.0), None),
                column![
                    text("Capri × Lora Companion & Widget Customizer")
                        .size(14)
                        .color(pal.text_primary),
                    text("Tune Capri's glitch crown shards, visor hue, eyes, toothy grin, widget scale, and chat/HUD docking.")
                        .size(11)
                        .color(pal.text_muted),
                    aura_row,
                ]
                .spacing(6)
                .width(Length::Fill),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
            row![
                text(format!("Crown Aura Hue: {}°", capri.crown_hue))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(185.0)),
                slider(0..=359, capri.crown_hue, Message::CapriCrownHueChanged)
                    .step(1_u16)
                    .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text(format!("Visor / Bolt Hue: {}°", capri.visor_hue))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(185.0)),
                slider(0..=359, capri.visor_hue, Message::CapriVisorHueChanged)
                    .step(1_u16)
                    .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text(format!("Glitch Spark Intensity: {}%", capri.glitch_intensity_pct))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(185.0)),
                slider(0_u8..=100, capri.glitch_intensity_pct, Message::CapriGlitchChanged)
                    .step(5_u8)
                    .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text(format!("Capri Widget Size: {} px", capri.widget_scale_px))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fixed(185.0)),
                slider(36_u16..=96, capri.widget_scale_px, Message::CapriScaleChanged)
                    .step(2_u16)
                    .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text("EYE EXPRESSION")
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(pal.text_muted),
                eye_row,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row![
                checkbox(capri.show_aura_shards)
                    .label("Jagged Crown Shards")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::CapriToggleCrown),
                checkbox(capri.show_lightning)
                    .label("Neon Lightning Bolts")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::CapriToggleLightning),
                checkbox(capri.always_toothy_grin)
                    .label("Permanent Toothy Grin")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::CapriToggleGrin),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
            row![
                checkbox(capri.dock_widget_in_chat)
                    .label("Dock Capri Companion Widget in #chat")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::CapriToggleDockInChat),
                checkbox(capri.floating_badge_chip)
                    .label("Show Header Capri Status Chip")
                    .size(15)
                    .text_size(12)
                    .on_toggle(Message::CapriToggleHeaderChip),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        ]
        .spacing(10),
    );

    let governance_card = section_card(
        pal,
        rad,
        16,
        column![
            text("Governance, Attestation & Profile Controls")
                .size(14)
                .color(pal.text_primary),
            checkbox(app.experimental_enabled)
                .label("Enable experimental lab sections (Adapter Studio & Extension Host)")
                .size(15)
                .text_size(12)
                .on_toggle(Message::ToggleExperimental),
            row![
                button(text("Capture & Verify DesktopSnapshot").size(12))
                    .padding(Padding::from([8, 14]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_primary,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::CaptureAttestedSnapshotPressed),
                button(text("Preview Consent Modal").size(12))
                    .padding(Padding::from([8, 14]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.warning,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.warning,
                        c_rad,
                    ))
                    .on_press(Message::PreviewConsentDialogPressed),
                space::horizontal(),
                button(text("Reset All Studio & Layout").size(12))
                    .padding(Padding::from([8, 14]))
                    .style(theme::solid_button_with_radius(
                        Color {
                            a: 0.16,
                            ..pal.critical
                        },
                        pal.critical,
                        pal.text_primary,
                        c_rad,
                    ))
                    .on_press(Message::ResetDashboardLayout),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            rule::horizontal(1).style(theme::palette_divider_rule(pal)),
            text(format!(
                "Lora Agent Desktop · Theme={} · Corner={}px · Glow={}%",
                app.dashboard.active_theme_code(),
                prefs.corner_radius_px,
                prefs.glow_strength_pct
            ))
            .size(11)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
        ]
        .spacing(10),
    );

    scrollable(
        column![
            custom_matrix_card,
            curated_gallery,
            shape_chrome,
            capri_studio_card,
            governance_card
        ]
        .spacing(12)
        .padding(18),
    )
    .height(Length::Fill)
    .into()
}
