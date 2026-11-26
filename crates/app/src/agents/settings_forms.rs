use std::rc::Rc;

use agents::{AgentBackend, Environment, OutputFormat};
use app_base::ui_locale::tr_with;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, h_flex, input::Textarea, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::settings_controls::{
    Choice, MenuEntry, PAGE_PADDING, boxed, dropdown, field, hint, raised_button, text_field,
};
use super::settings_problems::{agent_problem, command_problem, placeholder_problem};
use super::settings_section::{AgentsSettings, Probe};
use crate::i18n::tr;
use crate::settings::switch;
use crate::theme;
use crate::views::widgets::icon;

const PLACEHOLDER_CHIPS: [&str; 9] = [
    "message",
    "sender",
    "chat",
    "thread",
    "extra",
    "conversation_id",
    "message_id",
    "session_id",
    "agent",
];

fn host_label() -> String {
    if cfg!(windows) {
        "Windows".to_owned()
    } else {
        tr("ui.settings.agents.this_computer")
    }
}

fn environment_choices() -> [Environment; 3] {
    [
        Environment::Host,
        Environment::Wsl { distro: None },
        Environment::Wsl {
            distro: Some(String::new()),
        },
    ]
}

fn environment_label(environment: &Environment) -> String {
    match environment {
        Environment::Host => host_label(),
        Environment::Wsl { distro: None } => tr("ui.settings.agents.wsl_default"),
        Environment::Wsl { distro: Some(_) } => tr("ui.settings.agents.wsl_named"),
    }
}

fn environment_kind(environment: &Environment) -> u8 {
    match environment {
        Environment::Host => 0,
        Environment::Wsl { distro: None } => 1,
        Environment::Wsl { distro: Some(_) } => 2,
    }
}

fn same_environment_kind(left: &Environment, right: &Environment) -> bool {
    environment_kind(left) == environment_kind(right)
}

fn output_choices() -> [(&'static str, OutputFormat); 3] {
    [
        (
            "ui.settings.agents.output_claude_stream",
            OutputFormat::ClaudeStreamJson,
        ),
        (
            "ui.settings.agents.output_copilot_json",
            OutputFormat::CopilotJsonl,
        ),
        ("ui.settings.agents.output_plain_text", OutputFormat::Text),
    ]
}

fn back_row(title: String, cx: &mut Context<AgentsSettings>) -> Stateful<Div> {
    h_flex()
        .id("agents-back")
        .w_full()
        .px(px(PAGE_PADDING))
        .pt(px(12.))
        .pb(px(8.))
        .gap(px(6.))
        .items_center()
        .cursor_pointer()
        .text_size(px(15.))
        .font_weight(FontWeight::BOLD)
        .child(icon(IconName::ArrowLeft, 16., theme::text()))
        .child(div().truncate().child(title))
        .on_click(cx.listener(|this, _, _, cx| this.go_back(cx)))
}

fn form(children: impl IntoIterator<Item = AnyElement>) -> Div {
    v_flex()
        .w_full()
        .px(px(PAGE_PADDING))
        .pb(px(16.))
        .gap(px(12.))
        .children(children)
}

impl AgentsSettings {
    pub(super) fn agent_page(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Div {
        let (Some(form_state), Some(profile)) = (&self.agent_form, self.current_agent(cx)) else {
            return div();
        };
        let problems = self.runs.read(cx).settings().validate();
        let this = cx.entity();
        let environment_entries: Vec<MenuEntry> = environment_choices()
            .into_iter()
            .map(|environment| {
                let this = this.clone();
                let label = environment_label(&environment);
                let selected = same_environment_kind(&environment, &profile.environment);
                let choose: Choice = Rc::new(move |window, cx| {
                    let environment = environment.clone();
                    this.update(cx, |this, cx| this.set_environment(environment, window, cx))
                });
                MenuEntry {
                    label,
                    selected,
                    choose,
                }
            })
            .collect();
        let environment_text = environment_label(&profile.environment);
        let output_entries: Vec<MenuEntry> = output_choices()
            .into_iter()
            .map(|(key, output)| {
                let this = this.clone();
                let choose: Choice =
                    Rc::new(move |_, cx| this.update(cx, |this, cx| this.set_output(output, cx)));
                MenuEntry {
                    label: tr(key),
                    selected: output == profile.output,
                    choose,
                }
            })
            .collect();
        let output_label = output_choices()
            .into_iter()
            .find(|(_, output)| *output == profile.output)
            .map_or_else(String::new, |(key, _)| tr(key));
        let command_problem = command_problem(&problems, &profile.id, &profile.command);
        let open_problem =
            placeholder_problem(&problems, &profile.id, &profile.open_session_command);
        let named_distro = matches!(profile.environment, Environment::Wsl { distro: Some(_) });
        let chat = profile.is_chat();
        let status = match &self.probe {
            Probe::Idle => None,
            Probe::Running => Some((
                if chat {
                    tr("ui.settings.agents.loading_models")
                } else {
                    tr("ui.settings.agents.testing")
                },
                theme::text_muted(),
            )),
            Probe::Found(line) => Some((
                if chat {
                    line.clone()
                } else {
                    tr_with("ui.settings.agents.found", &[("line", line)])
                },
                theme::green(),
            )),
            Probe::Missing(error) => Some((error.clone(), theme::red_soft())),
        };
        let shows_base = matches!(profile.backend, AgentBackend::OpenAi { .. });
        let key_saved = match &profile.backend {
            AgentBackend::OpenAi { api_key, .. } | AgentBackend::Anthropic { api_key, .. } => {
                !api_key.is_empty()
            }
            AgentBackend::Command => false,
        };
        let model_problem = problems.iter().find_map(|problem| match problem {
            agents::SettingsProblem::EmptyModel { agent_id } if agent_id == &profile.id => {
                Some(tr("ui.settings.agents.choose_model"))
            }
            _ => None,
        });
        let current_model = form_state.model.read(cx).value().trim().to_owned();
        let mut model_names = self.models.clone();
        if !current_model.is_empty() && !model_names.iter().any(|name| name == &current_model) {
            model_names.insert(0, current_model.clone());
        }
        let model_entries: Vec<MenuEntry> = model_names
            .iter()
            .map(|name| {
                let this = this.clone();
                let label = name.clone();
                let chosen = label.clone();
                let selected = label == current_model;
                let choose: Choice = Rc::new(move |window, cx| {
                    let name = chosen.clone();
                    this.update(cx, |this, cx| this.set_model(name, window, cx))
                });
                MenuEntry {
                    label,
                    selected,
                    choose,
                }
            })
            .collect();
        let model_label = if current_model.is_empty() {
            tr("ui.settings.agents.choose_model")
        } else {
            current_model
        };
        let test_this = this.clone();
        let delete_this = this.clone();
        v_flex()
            .w_full()
            .child(back_row(profile.name.clone(), cx))
            .child(form([
                field(
                    tr("ui.settings.agents.field_name"),
                    text_field(&form_state.name, false, false, cx),
                    None,
                    None,
                )
                .into_any_element(),
                div()
                    .w_full()
                    .when(chat && shows_base, |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_base_url"),
                            text_field(&form_state.base_url, true, false, cx),
                            Some(hint(tr("ui.settings.agents.hint_base_url"))),
                            None,
                        ))
                    })
                    .into_any_element(),
                div()
                    .w_full()
                    .when(chat && !model_entries.is_empty(), |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_installed_models"),
                            dropdown("agent-model", model_label, model_entries),
                            Some(hint(tr("ui.settings.agents.hint_installed_models"))),
                            None,
                        ))
                    })
                    .into_any_element(),
                div()
                    .w_full()
                    .when(chat, |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_model"),
                            text_field(&form_state.model, true, model_problem.is_some(), cx),
                            Some(hint(tr("ui.settings.agents.hint_model"))),
                            model_problem.clone(),
                        ))
                    })
                    .into_any_element(),
                div()
                    .w_full()
                    .when(chat, |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_api_key"),
                            text_field(&form_state.api_key, true, false, cx),
                            Some(hint(if key_saved {
                                tr("ui.settings.agents.hint_api_key_saved")
                            } else {
                                tr("ui.settings.agents.hint_api_key")
                            })),
                            None,
                        ))
                    })
                    .into_any_element(),
                div()
                    .w_full()
                    .when(!chat, |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_runs_in"),
                            dropdown(
                                "agent-environment",
                                environment_text.to_owned(),
                                environment_entries,
                            ),
                            Some(hint(tr("ui.settings.agents.hint_runs_in"))),
                            None,
                        ))
                    })
                    .into_any_element(),
                div()
                    .w_full()
                    .when(!chat && named_distro, |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_distro"),
                            text_field(&form_state.distro, false, false, cx),
                            None,
                            None,
                        ))
                    })
                    .into_any_element(),
                div()
                    .w_full()
                    .when(!chat, |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_command"),
                            text_field(&form_state.command, true, command_problem.is_some(), cx),
                            Some(hint(tr("ui.settings.agents.hint_command"))),
                            command_problem,
                        ))
                    })
                    .into_any_element(),
                div()
                    .w_full()
                    .when(!chat, |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_output"),
                            dropdown("agent-output", output_label.to_owned(), output_entries),
                            Some(hint(tr("ui.settings.agents.hint_output"))),
                            None,
                        ))
                    })
                    .into_any_element(),
                div()
                    .w_full()
                    .when(!chat, |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_mcp_flag"),
                            text_field(&form_state.mcp_flag, true, false, cx),
                            Some(hint(tr("ui.settings.agents.hint_mcp_flag"))),
                            None,
                        ))
                    })
                    .into_any_element(),
                div()
                    .w_full()
                    .when(!chat, |slot| {
                        slot.child(field(
                            tr("ui.settings.agents.field_open_session"),
                            text_field(&form_state.open_session, true, open_problem.is_some(), cx),
                            Some(hint(tr("ui.settings.agents.hint_open_session"))),
                            open_problem,
                        ))
                    })
                    .into_any_element(),
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        raised_button("agent-test", tr("ui.settings.agents.test")).on_click(
                            move |_, _, cx| test_this.update(cx, |this, cx| this.test_agent(cx)),
                        ),
                    )
                    .children(status.map(|(text, color)| {
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(theme::text_small())
                            .text_color(color)
                            .child(text)
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("agent-delete")
                            .cursor_pointer()
                            .text_size(theme::text_label())
                            .text_color(theme::red_soft())
                            .child(tr("common.delete"))
                            .on_click(move |_, _, cx| {
                                delete_this.update(cx, |this, cx| this.delete_agent(cx))
                            }),
                    )
                    .into_any_element(),
            ]))
    }

    pub(super) fn action_page(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(form_state) = &self.action_form else {
            return div();
        };
        let settings = self.runs.read(cx).settings().clone();
        let Some(action) = settings
            .actions
            .iter()
            .find(|action| action.id == form_state.id)
            .cloned()
        else {
            return div();
        };
        let problems = settings.validate();
        let this = cx.entity();
        let agent_entries: Vec<MenuEntry> = settings
            .agents
            .iter()
            .map(|agent| {
                let (this, agent_id) = (this.clone(), agent.id.clone());
                let selected = agent.id == action.agent_id;
                let choose: Choice = Rc::new(move |_, cx| {
                    let agent_id = agent_id.clone();
                    this.update(cx, |this, cx| this.set_action_agent(agent_id, cx))
                });
                MenuEntry {
                    label: agent.name.clone(),
                    selected,
                    choose,
                }
            })
            .collect();
        let agent_label = settings
            .agents
            .iter()
            .find(|agent| agent.id == action.agent_id)
            .map_or_else(
                || tr("ui.settings.agents.choose_agent"),
                |agent| agent.name.clone(),
            );
        let agent_problem = agent_problem(&problems, &action.id);
        let prompt_problem = placeholder_problem(&problems, &action.id, &action.prompt_template);
        let args_problem = placeholder_problem(&problems, &action.id, &action.extra_args);
        let chips =
            h_flex()
                .w_full()
                .flex_wrap()
                .gap(px(4.))
                .children(PLACEHOLDER_CHIPS.iter().map(|placeholder| {
                    let this = this.clone();
                    div()
                        .id(*placeholder)
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .bg(theme::row_hover())
                        .hover(|chip| chip.bg(theme::surface_raised()))
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(theme::text_caption())
                        .text_color(theme::accent_text())
                        .child(format!("{{{placeholder}}}"))
                        .on_click(move |_, window, cx| {
                            this.update(cx, |this, cx| {
                                this.insert_placeholder(placeholder, window, cx)
                            })
                        })
                }));
        let ask = action.ask_extra_instructions;
        let (ask_this, delete_this) = (this.clone(), this.clone());
        v_flex()
            .w_full()
            .child(back_row(action.label.clone(), cx))
            .child(form([
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .items_start()
                    .child(
                        field(
                            tr("ui.settings.agents.field_label"),
                            text_field(&form_state.label, false, false, cx),
                            None,
                            None,
                        )
                        .flex_1()
                        .min_w(px(0.)),
                    )
                    .child(
                        field(
                            tr("ui.settings.agents.field_agent"),
                            dropdown("action-agent", agent_label, agent_entries),
                            None,
                            agent_problem,
                        )
                        .flex_1()
                        .min_w(px(0.)),
                    )
                    .into_any_element(),
                field(
                    tr("agents.start.working_dir"),
                    text_field(&form_state.working_dir, true, false, cx),
                    Some(hint(tr("ui.settings.agents.hint_working_dir"))),
                    None,
                )
                .into_any_element(),
                field(
                    tr("ui.settings.agents.field_prompt"),
                    boxed(
                        Textarea::new(&form_state.prompt)
                            .appearance(false)
                            .bordered(false),
                        prompt_problem.is_some(),
                        cx,
                    ),
                    Some(chips.into_any_element()),
                    prompt_problem,
                )
                .into_any_element(),
                h_flex()
                    .id("action-ask-extra")
                    .w_full()
                    .gap(px(12.))
                    .items_center()
                    .cursor_pointer()
                    .child(
                        v_flex()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(theme::text_body())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tr("ui.settings.agents.ask_extra")),
                            )
                            .child(
                                div()
                                    .text_size(theme::text_small())
                                    .text_color(theme::text_muted())
                                    .child(tr("ui.settings.agents.ask_extra_detail")),
                            ),
                    )
                    .child(switch(ask))
                    .on_click(move |_, _, cx| {
                        ask_this.update(cx, |this, cx| this.set_ask_extra(!ask, cx))
                    })
                    .into_any_element(),
                field(
                    tr("ui.settings.agents.field_extra_args"),
                    text_field(&form_state.extra_args, true, args_problem.is_some(), cx),
                    Some(hint(tr("ui.settings.agents.hint_extra_args"))),
                    args_problem,
                )
                .into_any_element(),
                h_flex()
                    .w_full()
                    .justify_end()
                    .child(
                        div()
                            .id("action-delete")
                            .cursor_pointer()
                            .text_size(theme::text_label())
                            .text_color(theme::red_soft())
                            .child(tr("common.delete"))
                            .on_click(move |_, _, cx| {
                                delete_this.update(cx, |this, cx| this.delete_action(cx))
                            }),
                    )
                    .into_any_element(),
            ]))
    }
}

pub(super) fn resume_toggle(resume: bool, settings: Entity<AgentsSettings>) -> Stateful<Div> {
    h_flex()
        .id("resume-interrupted-runs")
        .w_full()
        .px(px(PAGE_PADDING))
        .pt(px(16.))
        .gap(px(12.))
        .items_center()
        .cursor_pointer()
        .child(
            v_flex()
                .flex_1()
                .child(
                    div()
                        .text_size(theme::text_body())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(tr("ui.settings.agents.resume_title")),
                )
                .child(
                    div()
                        .text_size(theme::text_small())
                        .text_color(theme::text_muted())
                        .child(tr("ui.settings.agents.resume_detail")),
                ),
        )
        .child(switch(resume))
        .on_click(move |_, _, cx| {
            settings.update(cx, |this, cx| this.set_resume_interrupted_runs(!resume, cx))
        })
}

#[cfg(test)]
mod tests {
    use agents::Environment;

    use super::same_environment_kind;

    #[test]
    fn environments_compare_by_kind_not_distro_name() {
        let named = |name: &str| Environment::Wsl {
            distro: Some(name.to_owned()),
        };
        assert!(same_environment_kind(&named("Ubuntu"), &named("Debian")));
        assert!(!same_environment_kind(
            &named("Ubuntu"),
            &Environment::Wsl { distro: None }
        ));
        assert!(!same_environment_kind(&Environment::Host, &named("Ubuntu")));
        assert!(same_environment_kind(
            &Environment::Host,
            &Environment::Host
        ));
    }
}
