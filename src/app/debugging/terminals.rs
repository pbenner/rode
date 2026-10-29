//! Terminal transport and backpressure, separate from debugger state and UI.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
impl EditorApp {
    pub(super) fn handle_debug_terminal(
        &mut self,
        event: DebugEvent,
        client: &mut Client,
        ctx: &egui::Context,
    ) -> Option<DebugEvent> {
        match event {
            DebugEvent::Terminal { id, title } => {
                if self.debugger.streams.len() >= 4 || self.sessions.view_count() >= 16 {
                    client.error =
                        "Close a terminal view before opening another debug terminal.".into();
                    client.stop();
                    return None;
                }
                if let Some(writer) = client.writer() {
                    let (terminal, stream) = Terminal::stream(
                        ctx,
                        format!("DEBUG · {title}"),
                        move |event| {
                            use crate::terminal::StreamControl;
                            let ending = matches!(event, StreamControl::End);
                            let message = match event {
                                StreamControl::Input(bytes) => {
                                    serde_json::json!({"op":"input","id":id,"data":STANDARD.encode(bytes)})
                                }
                                StreamControl::Resize(rows, columns) => {
                                    let _=writer.try_send(&serde_json::json!({"op":"resize","id":id,"rows":rows,"columns":columns}));
                                    return;
                                }
                                StreamControl::End => {
                                    serde_json::json!({"op":"end_terminal","id":id})
                                }
                            };
                            let writer = writer.clone();
                            let send = move || {
                                for _ in 0..100 {
                                    match writer.try_send(&message) {
                                        Ok(false) => std::thread::sleep(Duration::from_millis(10)),
                                        _ => break,
                                    }
                                }
                            };
                            if ending {
                                std::thread::spawn(send);
                            } else {
                                send();
                            }
                        },
                    );
                    let key = format!("debug-{}-{id}", self.debugger.serial);
                    self.add_terminal_view(terminal, key.clone(), format!("Debug · {title}"));
                    self.debugger.streams.insert(id, (key, stream));
                }
            }
            DebugEvent::Data { id, data } => {
                if let Some((_, stream)) = self.debugger.streams.get(&id) {
                    match STANDARD.decode(&data) {
                        Ok(bytes) => match stream.feed(bytes) {
                            Ok(()) => {}
                            Err(mpsc::TrySendError::Full(_)) => {
                                // Retain this bounded envelope and stop reading the transport
                                // until the parser catches up. Backpressure reaches the PTY.
                                return Some(DebugEvent::Data { id, data });
                            }
                            Err(mpsc::TrySendError::Disconnected(_)) => stream.end(),
                        },
                        Err(error) => {
                            client.error = error.to_string();
                            client.stop();
                        }
                    }
                }
            }
            DebugEvent::TerminalExited { id } => {
                if let Some((_, stream)) = self.debugger.streams.get(&id) {
                    stream.end();
                }
            }
            _ => {}
        }
        None
    }
}
