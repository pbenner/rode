//! Asynchronous questions arrive as agent messages, not server RPC requests.
//! Answers steer a running turn or start a normal turn after it finishes. Keep
//! forms until acknowledgement; never invent an RPC reply or retry a model turn.
use super::*;

impl Chat {
    /// Normalize an asynchronous question item into a bounded, deduplicated form.
    /// Preserve matching draft answers and ignore cancelled or already-seen messages.
    pub(super) fn message_questions(&mut self, item: &Value) {
        let Some(item_id) = item["id"]
            .as_str()
            .filter(|id| controls::identifier(id, false))
        else {
            return;
        };
        let Some(values) = item["questions"]
            .as_array()
            .filter(|values| !values.is_empty())
        else {
            return;
        };
        if self.cancelling {
            return;
        }
        // Object IDs are local form identities and cannot collide with valid
        // JSON-RPC string/numeric request IDs. They are never sent as RPC IDs.
        let id = json!({"asyncQuestion":item_id});
        let previous = self
            .prompts
            .iter()
            .position(|prompt| prompt.async_message && prompt.id == id);
        if previous.is_none() && self.question_messages.iter().any(|seen| seen == item_id) {
            return;
        }
        let questions: Vec<_> = values
            .iter()
            .take(8)
            .enumerate()
            .filter_map(|(index, value)| {
                let title = value["title"]
                    .as_str()
                    .filter(|title| !title.trim().is_empty())?;
                let text = clip(title, 2048);
                let question_id = format!("question-{index}");
                let previous_answer = previous.and_then(|position| {
                    self.prompts[position]
                        .questions
                        .iter()
                        .find(|question| question.id == question_id && question.text == text)
                });
                Some(Question {
                    id: question_id,
                    text,
                    header: String::new(),
                    options: value["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .take(16)
                        .filter_map(|option| {
                            option
                                .as_str()
                                .filter(|s| !s.trim().is_empty())
                                .map(|s| clip(s, 1024))
                        })
                        .collect(),
                    descriptions: vec![],
                    answer: previous_answer
                        .map(|q| q.answer.clone())
                        .unwrap_or_default(),
                    answered: previous_answer.is_some_and(|q| q.answered),
                    secret: false,
                })
            })
            .collect();
        if questions.is_empty() {
            return;
        }
        if let Some(position) = previous {
            if !self.prompts[position].sending {
                self.prompts[position].questions = questions;
            }
        } else {
            if self.prompts.len() >= 16 {
                self.notice("More agent questions arrived than can be displayed. Answer or dismiss the existing questions first.");
                return;
            }
            self.question_messages.push_back(item_id.into());
            while self.question_messages.len() > 128 {
                self.question_messages.pop_front();
            }
            self.prompts.push_back(Prompt {
                id,
                title: "Agent asks a question".into(),
                summary: String::new(),
                detail: clip(&pretty(&item["questions"]), 16384),
                choices: vec![],
                questions,
                blocking: false,
                sending: false,
                error: String::new(),
                partial_answers: true,
                async_message: true,
                cancel: Value::Null,
            });
        }
        self.changed = true;
        self.request_status();
    }

    /// Validate and queue answers as a steer or new turn, retaining the form until acknowledged.
    /// A successful send marks delivery pending; it does not establish backend acceptance.
    pub(super) fn answer_message(&mut self, result: Value) -> Result<()> {
        ensure!(
            !self.cancelling && self.connected(),
            "The agent is stopping or disconnected"
        );
        let prompt = self.prompts.front().context("No current agent question")?;
        ensure!(!prompt.sending, "This answer is already being sent");
        let answers = result["answers"]
            .as_object()
            .context("Enter an answer before sending")?;
        ensure!(
            !answers.is_empty()
                && answers
                    .keys()
                    .all(|id| prompt.questions.iter().any(|q| &q.id == id && !q.answered))
                && (prompt.partial_answers
                    || answers.len() == prompt.questions.iter().filter(|q| !q.answered).count()),
            "Enter answers for the pending questions before sending"
        );
        let mut text = "Answers to your questions:\n".to_owned();
        for question in &prompt.questions {
            let Some(value) = answers.get(&question.id) else {
                continue;
            };
            let values = value["answers"]
                .as_array()
                .context("Answer every question before sending")?;
            ensure!(values.len() == 1, "Choose one answer or enter your own");
            let answer = values[0]
                .as_str()
                .filter(|answer| !answer.trim().is_empty())
                .context("Answer every question before sending")?;
            text.push_str(&format!("\n{}\n{answer}\n", question.text));
        }
        attachment::validate(&text, &[])?;
        let id = prompt.id.clone();
        let turn = if self.busy {
            ensure!(
                !self.turn.is_empty(),
                "Wait for the active turn to be ready before answering"
            );
            Some(self.turn.clone())
        } else {
            None
        };
        let call = Call::Answer {
            id: id.clone(),
            text: text.clone(),
            answers: answers.keys().cloned().collect(),
            turn: turn.clone(),
            deadline: Some(Instant::now() + Duration::from_secs(30)),
        };
        let input = self.prompt_input(text, vec![]);
        let sent = if let Some(turn) = &turn {
            self.call(
                "turn/steer",
                json!({"threadId":self.conversation,"expectedTurnId":turn,"input":input}),
                call,
            )
        } else {
            self.busy = true;
            self.last_turn_succeeded = false;
            self.file_calls = 0;
            self.codex_turn(input, call)
        };
        if let Err(error) = sent {
            self.answer_failed(&id, &format!("{error:#}"), turn.is_none());
            return Err(error);
        }
        if let Some(prompt) = self.prompts.iter_mut().find(|prompt| prompt.id == id) {
            prompt.sending = true;
            prompt.error.clear();
        }
        self.changed = true;
        self.request_status();
        Ok(())
    }

    /// Keep drafts after a failed answer send, clear its pending flag and expose a retry explanation.
    /// Only reset new-turn busy state when no backend turn identity was established.
    pub(super) fn answer_failed(&mut self, id: &Value, message: &str, new_turn: bool) {
        if new_turn && self.turn.is_empty() {
            self.busy = false;
        }
        let explanation = crate::network::explain_error(
            self.config.provider,
            self.controller_offline,
            message,
            1536,
        );
        if let Some(prompt) = self.prompts.iter_mut().find(|prompt| &prompt.id == id) {
            prompt.sending = false;
            prompt.error = clip(
                &format!(
                    "Answer delivery was not confirmed: {explanation}. Your answer is kept. Review the agent's output before trying again."
                ),
                2048,
            );
        }
        self.changed = true;
        if !self.cancelling {
            self.request_status();
        }
    }

    /// Apply an answer acknowledgement only when its returned turn identity is valid.
    /// Mark acknowledged questions complete and add the accepted text to transcript/recall.
    pub(super) fn answer_accepted(
        &mut self,
        id: Value,
        text: String,
        answers: Vec<String>,
        turn: Option<String>,
        result: &Value,
    ) -> Result<()> {
        let returned = if turn.is_some() {
            result["turnId"].as_str()
        } else {
            result["turn"]["id"].as_str()
        };
        let Some(returned) = returned
            .filter(|value| controls::identifier(value, false))
            .filter(|value| turn.as_deref().is_none_or(|expected| expected == *value))
        else {
            self.answer_unconfirmed(&id, "The agent returned an invalid acknowledgement");
            return Ok(());
        };
        if turn.is_none() && self.busy {
            self.turn = returned.into();
            if self.cancelling {
                self.interrupt()?;
            }
        }
        if let Some(prompt) = self.prompts.iter_mut().find(|prompt| prompt.id == id) {
            for question in &mut prompt.questions {
                if answers.contains(&question.id) {
                    question.answered = true;
                }
            }
            prompt.sending = false;
            prompt.error.clear();
        }
        self.prompts
            .retain(|prompt| prompt.id != id || prompt.questions.iter().any(|q| !q.answered));
        self.serial += 1;
        let message_id = format!("user-{}", self.serial);
        self.input_history.remember(&message_id, &text);
        self.push(message_id, Role::User, "You".into(), text);
        self.changed = true;
        if !self.cancelling {
            self.request_status();
        }
        Ok(())
    }

    /// Poll answer deadlines without blocking or resubmitting input.
    /// An expired acknowledgement retains its request identity and pending form for a late reply.
    pub(super) fn poll_answers(&mut self) {
        let expired: Vec<_> = self
            .calls
            .iter()
            .filter_map(|(id, call)| {
                matches!(call, Call::Answer { deadline: Some(deadline), .. } if *deadline <= Instant::now())
                    .then_some(*id)
            })
            .collect();
        for request_id in expired {
            if let Some(Call::Answer { id, deadline, .. }) = self.calls.get_mut(&request_id) {
                // Expiry is not evidence of rejection: the backend may already
                // be executing this input. Retain its identity for a late reply,
                // keep submission disabled, and stop polling this deadline.
                *deadline = None;
                let id = id.clone();
                self.answer_unconfirmed(&id, "The acknowledgement timed out");
            }
        }
    }

    fn answer_unconfirmed(&mut self, id: &Value, message: &str) {
        if let Some(prompt) = self.prompts.iter_mut().find(|prompt| &prompt.id == id) {
            prompt.sending = true;
            prompt.error = clip(
                &format!(
                    "Answer delivery is still unconfirmed: {message}. The answer will not be sent again. You can Stop while the agent is working, or dismiss this question after the turn finishes."
                ),
                2048,
            );
        }
        self.changed = true;
    }
}

#[cfg(test)]
mod tests;
