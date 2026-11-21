//! Incoming Codex/ACP routing and normalization into the shared conversation model.
//!
//! Admit notifications and server requests only for the captured conversation;
//! correlate responses with pending calls before updating lifecycle or question state.
//! Tool permission requests still use the controller's existing access/approval policy.
use super::*;

impl Chat {
    pub(super) fn receive(&mut self, value: Value) -> Result<()> {
        if let Some(method) = value["method"].as_str() {
            let params = &value["params"];
            if let Some(id) = value.get("id") {
                return self.request(method, id.clone(), params);
            }
            if self.config.provider.is_acp() {
                // session/load may replay saved messages before its response.
                // Recovery retains our transcript; only accept live updates once
                // the returned conversation ID has been validated and adopted.
                if method == "session/update"
                    && params["sessionId"].as_str() == Some(&self.conversation)
                {
                    self.acp_update(&params["update"]);
                }
            } else if params
                .get("threadId")
                .and_then(Value::as_str)
                .is_none_or(|id| id == self.conversation)
            {
                self.codex_update(method, params)?;
            }
            return Ok(());
        }
        let Some(id) = value["id"].as_u64() else {
            return Ok(());
        };
        let Some(call) = self.calls.remove(&id) else {
            return Ok(());
        };
        if let Some(error) = value.get("error") {
            if let Call::Answer { id, turn, .. } = call {
                self.answer_failed(
                    &id,
                    error["message"]
                        .as_str()
                        .unwrap_or("Answer was not accepted"),
                    turn.is_none(),
                );
                return Ok(());
            }
            if matches!(call, Call::Models) {
                let message = error["message"].as_str().unwrap_or("request failed");
                let explanation = crate::network::explain_error(
                    self.config.provider,
                    self.controller_offline,
                    message,
                    16384,
                );
                if explanation != message {
                    self.notice(&explanation);
                    return Ok(());
                }
                self.notice("This agent does not provide a model catalog. Enter a model ID in Model controls.");
                return Ok(());
            }
            // A completed turn may race its interrupt response. Keep the
            // conversation; the cancellation deadline handles an unresponsive agent.
            if matches!(call, Call::Interrupt) {
                return Ok(());
            }
            anyhow::bail!(
                "Agent: {}",
                error["message"].as_str().unwrap_or("request failed")
            );
        }
        let result = &value["result"];
        match call {
            Call::Answer {
                id,
                text,
                answers,
                turn,
                ..
            } => self.answer_accepted(id, text, answers, turn, result)?,
            Call::Initialize => {
                if self.config.provider.is_acp() {
                    ensure!(
                        result["protocolVersion"].as_u64() == Some(1),
                        "Agent does not support ACP v1."
                    );
                    self.image_support = result["agentCapabilities"]["promptCapabilities"]["image"]
                        .as_bool()
                        .unwrap_or(false);
                    if let Some(id) = self.resume_id().map(str::to_owned) {
                        ensure!(
                            result["agentCapabilities"]["loadSession"].as_bool() == Some(true),
                            "This ACP agent cannot reconnect saved sessions. Use Start new context to recover while keeping queued prompts."
                        );
                        self.call(
                            "session/load",
                            json!({"sessionId":id,"cwd":self.root,"mcpServers":[]}),
                            Call::Conversation,
                        )?;
                    } else {
                        self.call(
                            "session/new",
                            json!({"cwd":self.root,"mcpServers":[]}),
                            Call::Conversation,
                        )?;
                    }
                } else {
                    self.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
                    if self.config.provider == Provider::Codex {
                        self.call(
                            "model/list",
                            json!({"limit":32,"includeHidden":false}),
                            Call::Models,
                        )?;
                    }
                    if self.config.native_tools() {
                        self.call(
                            "config/read",
                            json!({"includeLayers":false}),
                            Call::FilePolicy,
                        )?;
                        return Ok(());
                    }
                    let access = if self.read_only {
                        Access::ReadOnly
                    } else {
                        self.config.access
                    };
                    let mut params = json!({"cwd":self.root,"approvalPolicy":if self.read_only {"never"} else {self.config.approval.codex()},"sandbox":access.codex()});
                    if !self.config.model().is_empty() {
                        params["model"] = self.config.model().into();
                    }
                    if self.config.provider == Provider::Local {
                        params["modelProvider"] = "rode_local".into();
                    }
                    self.start_conversation(params)?;
                }
            }
            Call::FilePolicy => {
                let config = crate::file_only::thread_config(&result["config"])?;
                self.start_file_job(
                    "rode_list_files",
                    json!({"path":"."}),
                    crate::file_only::Target::Verify(config),
                )?;
            }
            Call::Conversation => {
                let expected = self.resume_id().map(str::to_owned);
                let id = if self.config.provider.is_acp() {
                    result["sessionId"].as_str().or(expected.as_deref())
                } else {
                    result["thread"]["id"].as_str()
                }
                .filter(|id| !id.is_empty() && id.len() <= 256)
                .context("Agent returned no conversation ID")?;
                ensure!(
                    expected.as_deref().is_none_or(|expected| expected == id),
                    "Agent reconnected a different conversation; no prompt was sent"
                );
                self.conversation = id.into();
                if self.config.provider.is_acp() {
                    self.controls = controls::acp(result);
                }
                if self.recovery.take().is_some() {
                    if expected.is_none() {
                        // New processes may reuse item IDs. Retained messages
                        // must stay separate from output in the fresh context.
                        for (index, message) in self.messages.iter_mut().enumerate() {
                            message.id = format!("retained-{}-{index}", self.serial);
                        }
                    }
                    self.response_style_context = expected.is_some();
                    self.notice(if expected.is_some() {
                        "Agent reconnected to the saved conversation. The interrupted prompt was not repeated. Queued prompts remain paused."
                    } else {
                        "Agent restarted with a new context. Earlier messages remain visible, but are not in the new agent context. Queued prompts are kept and remain paused."
                    });
                    self.fresh_recovery = false;
                }
                self.begin_turn()?;
            }
            Call::Turn => {
                if self.config.provider.is_acp() {
                    self.finish(result["stopReason"].as_str() != Some("cancelled"), None);
                } else if self.busy {
                    self.turn = result["turn"]["id"]
                        .as_str()
                        .context("Agent returned no turn ID")?
                        .into();
                    if self.cancelling {
                        self.interrupt()?;
                    }
                }
            }
            Call::Interrupt => {}
            Call::Models => {
                if let Some(cursor) = self.model_pages.append(&mut self.models, result) {
                    self.call(
                        "model/list",
                        json!({"limit":32,"includeHidden":false,"cursor":cursor}),
                        Call::Models,
                    )?;
                }
            }
            Call::Configure { id, value } => {
                if self.cancelling {
                    self.finish(false, None);
                    return Ok(());
                }
                self.controls = controls::acp(result);
                ensure!(
                    self.controls
                        .iter()
                        .any(|c| c.id == id && c.current == value),
                    "Agent did not acknowledge the selected setting. No prompt was sent."
                );
                self.begin_turn()?;
            }
        }
        Ok(())
    }

    pub(super) fn codex_update(&mut self, method: &str, params: &Value) -> Result<()> {
        let item = &params["item"];
        match method {
            "turn/started" => {
                self.turn = params["turn"]["id"].as_str().unwrap_or_default().into();
            }
            "turn/completed" => {
                let status = params["turn"]["status"].as_str().unwrap_or("failed");
                self.finish(
                    status == "completed",
                    params["turn"]["error"]["message"].as_str(),
                );
            }
            "item/agentMessage/delta" => {
                self.append(
                    params["itemId"].as_str().unwrap_or("reply"),
                    Role::Assistant,
                    "Agent",
                    params["delta"].as_str().unwrap_or_default(),
                );
            }
            "item/commandExecution/outputDelta" => {
                self.append(
                    params["itemId"].as_str().unwrap_or("tool"),
                    Role::Tool,
                    "Command",
                    params["delta"].as_str().unwrap_or_default(),
                );
            }
            "item/started" | "item/completed" => {
                let id = item["id"].as_str().unwrap_or("item");
                match item["type"].as_str().unwrap_or_default() {
                    "agentMessage" => {
                        if let Some(text) = item["text"].as_str() {
                            self.replace(id, Role::Assistant, "Agent", text);
                        }
                        self.message_questions(item);
                    }
                    "commandExecution" => {
                        let title = item["command"].as_str().unwrap_or("Command");
                        let text = item["aggregatedOutput"].as_str().unwrap_or_default();
                        if method == "item/completed" {
                            self.replace(id, Role::Tool, title, text);
                        } else {
                            self.append(id, Role::Tool, title, "");
                        }
                    }
                    "fileChange" => {
                        self.replace(id, Role::Tool, "File changes", &pretty(&item["changes"]));
                        self.changed = true;
                    }
                    "mcpToolCall" | "dynamicToolCall" => self.replace(
                        id,
                        Role::Tool,
                        item["tool"].as_str().unwrap_or("Tool"),
                        &pretty(item),
                    ),
                    _ => {}
                }
                if method == "item/completed" {
                    self.tool_state(id, ToolState::codex_completed(item));
                }
            }
            "serverRequest/resolved" => {
                if params["threadId"]
                    .as_str()
                    .is_none_or(|id| id == self.conversation)
                {
                    let before = self.prompts.len();
                    self.prompts
                        .retain(|prompt| prompt.id != params["requestId"]);
                    if before != self.prompts.len() && self.busy && !self.cancelling {
                        self.request_status();
                    }
                }
            }
            "error" if !params["willRetry"].as_bool().unwrap_or(false) => {
                self.notice(
                    params["error"]["message"]
                        .as_str()
                        .unwrap_or("Agent reported an error"),
                );
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn acp_update(&mut self, update: &Value) {
        match update["sessionUpdate"].as_str().unwrap_or_default() {
            "agent_message_chunk" => {
                if let Some(text) = update["content"]["text"].as_str() {
                    let id = self
                        .messages
                        .back()
                        .filter(|m| m.role == Role::Assistant)
                        .map(|m| m.id.clone())
                        .unwrap_or_else(|| {
                            format!("reply-{}-{}", self.serial, self.messages.len())
                        });
                    self.append(&id, Role::Assistant, "Agent", text);
                }
            }
            "tool_call" | "tool_call_update" => {
                let id = update["toolCallId"].as_str().unwrap_or("tool");
                let title = update["title"].as_str().unwrap_or("Tool");
                self.replace(id, Role::Tool, title, &pretty(update));
                if let Some(state) = update["status"].as_str().and_then(ToolState::acp) {
                    self.tool_state(id, state);
                }
                self.changed = true;
            }
            "config_option_update" => {
                self.controls = controls::acp(update);
            }
            _ => {}
        }
    }

    pub(super) fn request(&mut self, method: &str, id: Value, params: &Value) -> Result<()> {
        if method == "item/tool/call" {
            return self.file_request(id, params);
        }
        if let Some(scope) = params
            .get(if self.config.provider.is_acp() {
                "sessionId"
            } else {
                "threadId"
            })
            .and_then(Value::as_str)
            && scope != self.conversation
        {
            return self.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Request belongs to another conversation"}}));
        }
        let mut prompt = Prompt {
            id: id.clone(),
            title: "Agent permission".into(),
            summary: clip(
                params["command"]
                    .as_str()
                    .or_else(|| params["toolCall"]["title"].as_str())
                    .or_else(|| params["reason"].as_str())
                    .unwrap_or_default(),
                2048,
            ),
            detail: clip(&pretty(params), 16384),
            choices: vec![],
            questions: vec![],
            blocking: true,
            sending: false,
            error: String::new(),
            partial_answers: false,
            async_message: false,
            cancel: json!({"decision":"cancel"}),
        };
        let permission = matches!(
            method,
            "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/permissions/requestApproval"
                | "session/request_permission"
        );
        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                prompt.title = if method.contains("commandExecution") {
                    "Run command?"
                } else {
                    "Apply file changes?"
                }
                .into();
                prompt.choices = vec![
                    Choice {
                        label: "Allow once".into(),
                        result: json!({"decision":"accept"}),
                    },
                    Choice {
                        label: "Decline".into(),
                        result: json!({"decision":"decline"}),
                    },
                ];
                if let Some(decisions) = params["availableDecisions"].as_array() {
                    prompt
                        .choices
                        .retain(|c| decisions.contains(&c.result["decision"]));
                    if prompt.choices.is_empty() {
                        prompt.choices.push(Choice {
                            label: "Cancel request".into(),
                            result: json!({"decision":"cancel"}),
                        });
                    }
                }
            }
            "item/permissions/requestApproval" => {
                prompt.cancel = json!({"permissions":{},"scope":"turn"});
                prompt.title = "Additional agent permissions?".into();
                prompt.choices = vec![
                    Choice {
                        label: "Allow for this turn".into(),
                        result: json!({"permissions":params["permissions"],"scope":"turn"}),
                    },
                    Choice {
                        label: "Decline".into(),
                        result: json!({"permissions":{},"scope":"turn"}),
                    },
                ];
            }
            "item/tool/requestUserInput" => {
                prompt.cancel = json!({"answers":{}});
                prompt.title = "Agent needs your input".into();
                prompt.blocking = params["isBlocking"].as_bool().unwrap_or(true);
                if let Some(questions) = params["questions"].as_array() {
                    for question in questions.iter().take(8) {
                        prompt.questions.push(Question {
                            id: question["id"].as_str().unwrap_or_default().into(),
                            text: clip(question["question"].as_str().unwrap_or("Answer"), 2048),
                            header: clip(question["header"].as_str().unwrap_or_default(), 256),
                            options: question["options"]
                                .as_array()
                                .map(|v| {
                                    v.iter()
                                        .take(16)
                                        .filter_map(|o| o["label"].as_str().map(|s| clip(s, 1024)))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            descriptions: question["options"]
                                .as_array()
                                .map(|options| {
                                    options
                                        .iter()
                                        .take(16)
                                        .filter(|o| o["label"].is_string())
                                        .map(|o| {
                                            clip(
                                                o["description"].as_str().unwrap_or_default(),
                                                2048,
                                            )
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                            answer: String::new(),
                            answered: false,
                            secret: question["isSecret"].as_bool().unwrap_or(false),
                        });
                    }
                }
            }
            "session/request_permission" => {
                prompt.cancel = json!({"outcome":{"outcome":"cancelled"}});
                if let Some(options) = params["options"].as_array() {
                    for option in options.iter().take(8) {
                        let Some(option_id) = option["optionId"].as_str() else {
                            continue;
                        };
                        let label = match option["kind"].as_str().unwrap_or_default() {
                            "allow_once" => "Allow once",
                            "allow_always" => "Always allow",
                            "reject_once" => "Decline",
                            "reject_always" => "Always decline",
                            _ => continue,
                        };
                        prompt.choices.push(Choice {
                            label: label.into(),
                            result: json!({"outcome":{"outcome":"selected","optionId":option_id}}),
                        });
                    }
                }
            }
            "mcpServer/elicitation/request" => {
                prompt.cancel = json!({"action":"cancel"});
                prompt.title = "Agent requests information from another service".into();
                prompt.choices.push(Choice {
                    label: "Decline".into(),
                    result: json!({"action":"decline"}),
                });
            }
            _ => {
                self.notice(&format!("Agent requested unsupported client method {method}. Use an agent with its own filesystem/terminal tools."));
                return self.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"RODE does not provide client-side filesystem, terminal, authentication, or custom tool RPCs"}}));
            }
        }
        if permission
            && (self.config.approval == Approval::Never
                || self.config.native_tools()
                || self.read_only
                || self.cancelling
                || !self.busy)
        {
            let denied = if method == "session/request_permission" {
                params["options"]
                    .as_array()
                    .and_then(|options| options.iter().find(|o| o["kind"] == "reject_once"))
                    .and_then(|o| o["optionId"].as_str())
                    .map(|id| json!({"outcome":{"outcome":"selected","optionId":id}}))
                    .unwrap_or(prompt.cancel)
            } else if method == "item/permissions/requestApproval" {
                prompt.cancel
            } else {
                json!({"decision":if params["availableDecisions"].as_array().is_some_and(|choices| !choices.contains(&json!("decline"))) {"cancel"} else {"decline"}})
            };
            self.send(json!({"jsonrpc":"2.0","id":id,"result":denied}))?;
            self.notice("Permission request denied by the selected agent access/approval policy.");
            return Ok(());
        }
        ensure!(self.prompts.len() < 16, "Too many pending agent requests.");
        ensure!(
            !prompt.choices.is_empty() || !prompt.questions.is_empty(),
            "Agent sent a request with no supported choices."
        );
        self.prompts.push_back(prompt);
        self.request_status();
        Ok(())
    }

    /// Pending requests take precedence over the generic turn-running status.
    pub(super) fn request_status(&mut self) {
        self.status = if self.prompts.iter().any(|prompt| prompt.blocking) {
            "Waiting for you"
        } else if !self.prompts.is_empty() && self.busy {
            "Question awaiting your answer · agent working"
        } else if !self.prompts.is_empty() {
            "Question awaiting your answer"
        } else if self.busy {
            "Working…"
        } else {
            "Ready"
        }
        .into();
    }
}
