//! Attachment cursors keep generations and stream offsets, never old payloads.
//! A slow writer may hold a shared snapshot; subsequent frames carry only edits.
use super::*;

#[derive(Default)]
/// Compact replay position for one cell on one controlling attachment.
pub struct Cursor {
    revision: Option<u64>,
    epoch: u64,
    records: Vec<(u64, Option<usize>)>,
}
impl Cursor {
    /// Last delivered revision, absent before the initial full replay.
    pub fn revision(&self) -> Option<u64> {
        self.revision
    }
    /// Snapshot a new attachment, then publish incremental edits without retaining
    /// earlier stream strings or retransmitting unchanged rich output.
    pub fn update(&mut self, cell: &Cell) -> Option<Update> {
        if self.revision == Some(cell.revision) {
            return None;
        }
        let update = if self.revision.is_none() {
            Update::Cell(cell.clone())
        } else {
            let reset = self.epoch != cell.epoch || self.records.len() > cell.outputs.len();
            let mut edits = Vec::new();
            for (index, output) in cell.outputs.iter().enumerate() {
                let previous = (!reset).then(|| self.records.get(index)).flatten();
                match previous {
                    None => edits.push(OutputEdit::Append {
                        output: output.clone(),
                    }),
                    Some((generation, _)) if *generation != output.generation => {
                        edits.push(OutputEdit::Replace {
                            index,
                            output: output.clone(),
                        })
                    }
                    Some((_, Some(length))) => {
                        if let Some(text) = output.value["text"].as_str() {
                            if let Some(fragment) = text.get(*length..) {
                                if !fragment.is_empty() {
                                    edits.push(OutputEdit::Text {
                                        index,
                                        text: fragment.into(),
                                    });
                                }
                            } else {
                                edits.push(OutputEdit::Replace {
                                    index,
                                    output: output.clone(),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            Update::CellDelta(CellDelta {
                run: RunRef::from(&cell.run),
                revision: cell.revision,
                reset,
                edits,
                count: cell.count,
                done: cell.done.clone(),
            })
        };
        self.revision = Some(cell.revision);
        self.epoch = cell.epoch;
        self.records = cell
            .outputs
            .iter()
            .map(|output| {
                (
                    output.generation,
                    (output.value["output_type"] == "stream")
                        .then(|| output.value["text"].as_str().map(str::len))
                        .flatten(),
                )
            })
            .collect();
        Some(update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn replay_shares_large_payloads_and_later_prints_send_only_new_text() {
        let image = Output::new(
            json!({"data":{"text/html":"x".repeat(20*1024*1024)}}),
            Some("plot".into()),
        )
        .unwrap();
        let stream = Output::new(
            json!({"output_type":"stream","name":"stdout","text":"first\n"}),
            None,
        )
        .unwrap();
        let mut cell = Cell {
            run: notebook::Run {
                uid: 1,
                cell: "cell".into(),
                version: 0,
                code: "display(plot)".into(),
            },
            output_bytes: image.bytes + stream.bytes,
            outputs: vec![image, stream],
            count: Some(1),
            done: None,
            revision: 1,
            epoch: 1,
        };
        let mut cursor = Cursor::default();
        let first = cursor.update(&cell).unwrap();
        let Update::Cell(snapshot) = &first else {
            panic!()
        };
        assert!(Arc::ptr_eq(
            &snapshot.outputs[0].value,
            &cell.outputs[0].value
        ));
        drop(first);
        assert_eq!(
            Arc::strong_count(&cell.outputs[0].value),
            1,
            "Cursor must not retain old payloads"
        );
        Arc::make_mut(&mut cell.outputs[1].value)["text"] = json!("first\nsecond\n");
        cell.revision += 1;
        let next = cursor.update(&cell).unwrap();
        let Update::CellDelta(delta) = &next else {
            panic!()
        };
        assert!(
            matches!(delta.edits.as_slice(), [OutputEdit::Text {index:1,text}] if text=="second\n")
        );
        assert!(
            serde_json::to_vec(&next).unwrap().len() < 1024,
            "Unchanged HTML must not cross the wire again"
        );
        assert!(cursor.update(&cell).is_none());
        cell.epoch += 1;
        cell.revision += 1;
        cell.outputs.clear();
        cell.count = Some(2);
        let Some(Update::CellDelta(delta)) = cursor.update(&cell) else {
            panic!()
        };
        assert!(delta.reset && delta.edits.is_empty());
        cell.outputs
            .push(Output::new(json!({"data":{"text/plain":"new"}}), Some("plot".into())).unwrap());
        cell.revision += 1;
        let Some(Update::CellDelta(delta)) = cursor.update(&cell) else {
            panic!()
        };
        assert!(matches!(
            delta.edits.as_slice(),
            [OutputEdit::Append { .. }]
        ));
        cell.outputs[0] = Output::new(
            json!({"data":{"text/plain":"updated"}}),
            Some("plot".into()),
        )
        .unwrap();
        cell.revision += 1;
        let Some(Update::CellDelta(delta)) = cursor.update(&cell) else {
            panic!()
        };
        assert!(matches!(
            delta.edits.as_slice(),
            [OutputEdit::Replace { index: 0, .. }]
        ));
    }
}
