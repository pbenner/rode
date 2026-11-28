//! Atomic RAM admission and copy-on-write notebook payloads. Cached charges avoid
//! serializing every retained cell for each print, count or completion event.
use super::*;

pub(super) fn event(shared: &mut Shared, event: notebook::Event) -> Result<()> {
    use notebook::Event;
    let event = match event {
        Event::Output {
            output,
            update: true,
            display_id,
            ..
        } => {
            if let Some(id) = display_id {
                update_displays(
                    &mut shared.cells,
                    &id,
                    Arc::new(output),
                    &mut shared.output_bytes,
                    shared.view.output_budget,
                )?;
            }
            return Ok(());
        }
        Event::Ready(language) => {
            shared.view.language = language;
            return Ok(());
        }
        Event::Error(error) => {
            shared.view.error = error;
            shared.finished = true;
            return Ok(());
        }
        Event::Warning(warning) => {
            shared.view.error = warning;
            return Ok(());
        }
        event => event,
    };
    let run = match &event {
        Event::Output { run, .. }
        | Event::Clear(run)
        | Event::Count(run, _)
        | Event::Done(run, _) => run.clone(),
        _ => bail!("Unexpected host notebook event"),
    };
    let key = if run.cell.is_empty() {
        format!("uid:{}", run.uid)
    } else {
        run.cell.clone()
    };
    let previous = shared.cells.get(&key);
    ensure!(
        previous.is_some() || shared.cells.len() < 1024,
        "Notebook exceeds 1024 retained cells"
    );
    let reset = previous.is_none_or(|cell| {
        cell.run.uid != run.uid
            || cell.run.version != run.version
            || cell.run.code != run.code
            || cell.done.is_some()
    }) || matches!(event, Event::Clear(_));
    let old_bytes = previous.map_or(0, |cell| cell.output_bytes);
    let incoming = if let Event::Output {
        output, display_id, ..
    } = &event
    {
        let fragment = (!reset && display_id.is_none())
            .then(|| previous?.outputs.last())
            .flatten()
            .filter(|old| old.display.is_none())
            .and_then(|old| notebook::outputs::stream_fragment(&old.value, output));
        Some((
            fragment.is_some(),
            if let Some(text) = fragment {
                json_size(&text)?.saturating_sub(2)
            } else {
                notebook::outputs::retained_bytes(output)?
                    .saturating_add(display_id.as_ref().map_or(0, String::len))
            },
        ))
    } else {
        None
    };
    let candidate = shared
        .output_bytes
        .saturating_sub(if reset { old_bytes } else { 0 })
        .saturating_add(incoming.map_or(0, |(_, bytes)| bytes));
    if candidate > shared.output_bytes {
        shared.view.output_budget.check(candidate)?;
    }
    let revision = previous.map_or(1, |cell| cell.revision + 1);
    let cell = shared.cells.entry(key).or_insert_with(|| Cell {
        run: run.clone(),
        outputs: vec![],
        count: None,
        done: None,
        revision: 0,
        output_bytes: 0,
        epoch: revision,
    });
    if reset {
        cell.run = run;
        cell.outputs.clear();
        cell.output_bytes = 0;
        cell.count = None;
        cell.done = None;
        cell.epoch = revision;
    }
    match event {
        Event::Output {
            output, display_id, ..
        } => {
            let (merge, bytes) = incoming.unwrap();
            if merge {
                let old = cell.outputs.last_mut().unwrap();
                notebook::outputs::merge_stream(Arc::make_mut(&mut old.value), &output);
                old.bytes += bytes;
            } else {
                cell.outputs.push(service::Output::new(output, display_id)?);
            }
            cell.output_bytes += bytes;
        }
        Event::Count(_, count) => cell.count = Some(count),
        Event::Done(_, status) => cell.done = Some(status),
        _ => {}
    }
    cell.revision = revision;
    shared.output_bytes = candidate;
    Ok(())
}

/// Check the entire display fan-out before changing a slot, sharing the new MIME
/// payload between destinations while retaining atomic quota/error behavior.
pub(super) fn update_displays(
    cells: &mut BTreeMap<String, Cell>,
    id: &str,
    output: Arc<Value>,
    used: &mut usize,
    budget: notebook::outputs::MemoryBudget,
) -> Result<()> {
    let replacement = service::Output::shared(output, Some(id.into()))?;
    let mut candidate = *used;
    for cell in cells.values() {
        for old in &cell.outputs {
            if old.display.as_deref() == Some(id) {
                candidate = candidate
                    .saturating_sub(old.bytes)
                    .saturating_add(replacement.bytes);
            }
        }
    }
    if candidate > *used {
        budget.check(candidate)?;
    }
    for cell in cells.values_mut() {
        let mut changed = false;
        for old in &mut cell.outputs {
            if old.display.as_deref() == Some(id) {
                cell.output_bytes = cell
                    .output_bytes
                    .saturating_sub(old.bytes)
                    .saturating_add(replacement.bytes);
                *old = replacement.clone();
                changed = true;
            }
        }
        if changed {
            cell.revision += 1;
        }
    }
    *used = candidate;
    Ok(())
}
