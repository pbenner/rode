//! Bounded serial stdio serving. Domain errors belong in a native View; framing
//! failures close the process rather than trying to resynchronize unsafe input.
use crate::panel::{REPLY_LIMIT, REQUEST_LIMIT, Reply, Request};
use anyhow::{Result, ensure};
use std::io::{BufRead, Read, Write};
pub fn serve(
    input: impl BufRead,
    mut output: impl Write,
    mut handle: impl FnMut(Request) -> Reply,
) -> Result<()> {
    let mut input = input;
    loop {
        let mut line = Vec::new();
        if input
            .by_ref()
            .take((REQUEST_LIMIT + 1) as u64)
            .read_until(b'\n', &mut line)?
            == 0
        {
            break;
        }
        ensure!(
            line.len() <= REQUEST_LIMIT && line.ends_with(b"\n"),
            "Invalid or oversized panel frame"
        );
        let request: Request = serde_json::from_slice(&line)?;
        request.validate()?;
        let (api, id) = (request.api, request.id);
        let reply = handle(request);
        reply.validate(api, id)?;
        let bytes = serde_json::to_vec(&reply)?;
        ensure!(
            bytes.len() < REPLY_LIMIT,
            "Panel reply exceeds its size limit"
        );
        output.write_all(&bytes)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::View;
    #[test]
    fn serial_server_preserves_identity_and_rejects_truncated_and_oversized_frames() {
        let mut output = Vec::new();
        serve(
            std::io::Cursor::new(b"{\"api\":2,\"id\":7,\"action\":\"open\"}\n"),
            &mut output,
            |r| Reply {
                api: r.api,
                id: r.id,
                view: View {
                    title: "Test".into(),
                    ..Default::default()
                },
                settings: None,
            },
        )
        .unwrap();
        let reply: Reply = serde_json::from_slice(&output).unwrap();
        assert_eq!(reply.id, 7);
        for invalid in [b"{}".to_vec(), vec![b'x'; 1024 * 1024 + 1]] {
            assert!(
                serve(std::io::Cursor::new(invalid), Vec::new(), |_| panic!(
                    "Invalid framing must not dispatch work"
                ))
                .is_err()
            );
        }
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use crate::panel::{Action, View};

    #[test]
    fn malformed_requests_never_dispatch_and_bad_replies_never_publish() {
        for request in [
            serde_json::json!({"api":1,"id":1,"action":"open","settings":{"unexpected":true}}),
            serde_json::json!({"api":2,"id":1,"action":"tick","values":{"draft":"must not submit"}}),
            serde_json::json!({"api":2,"id":1,"action":"save","values":{"draft":"x".repeat(8193)}}),
        ] {
            let bytes = format!("{request}\n");
            assert!(
                serve(std::io::Cursor::new(bytes), Vec::new(), |_| panic!(
                    "Invalid input reached domain code"
                ))
                .is_err()
            );
        }
        for case in 0..4 {
            let mut output = Vec::new();
            let result = serve(
                std::io::Cursor::new(b"{\"api\":1,\"id\":7,\"action\":\"open\"}\n"),
                &mut output,
                |r| {
                    let mut reply = Reply {
                        api: r.api,
                        id: r.id,
                        view: View {
                            title: "Test".into(),
                            ..Default::default()
                        },
                        settings: None,
                    };
                    match case {
                        0 => reply.id += 1,
                        1 => reply.settings = Some(Default::default()),
                        2 => reply.view.actions.push(Action::new(
                            "tick",
                            "Invalid user action",
                            true,
                        )),
                        _ => reply.view.links.push(crate::panel::Link {
                            label: "V2 only".into(),
                            url: "https://example.org".into(),
                        }),
                    }
                    reply
                },
            );
            assert!(result.is_err());
            assert!(output.is_empty(), "Invalid reply leaked onto stdout");
        }
    }
}

#[cfg(test)]
mod wire_shape_tests {
    use super::*;
    #[test]
    fn empty_settings_are_explicit_in_v2_and_absent_in_v1() {
        for api in [1, 2] {
            let request = Request {
                api,
                id: 1,
                action: "open".into(),
                values: Default::default(),
                settings: Default::default(),
            };
            request.validate().unwrap();
            let wire = serde_json::to_value(&request).unwrap();
            assert_eq!(wire.get("settings").is_some(), api == 2);
            assert!(wire["values"].is_object());
        }
    }
}
