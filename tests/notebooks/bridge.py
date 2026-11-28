"""Exercise bridge accounting without requiring an installed Jupyter kernel."""

import contextlib
import io
import json
import sys
import unittest

bridge = {"__name__": "rode_notebook_bridge_test"}
exec(compile(sys.argv[1], "notebook_kernel.py", "exec"), bridge)
limits = {"bytes": 2048, "text": 256, "count": 4, "chunk_chars": 32}


class Manager:
    def __init__(self):
        self.interrupts = 0

    def interrupt_kernel(self):
        self.interrupts += 1


class Client:
    def __init__(self, events):
        self.events = iter(events)

    def execute(self, *_args, **_kwargs):
        return "execution"

    def get_iopub_msg(self, **_kwargs):
        kind, content = next(self.events)
        return {
            "parent_header": {"msg_id": "execution"},
            "msg_type": kind,
            "content": content,
        }

    def get_shell_msg(self, **_kwargs):
        return {"parent_header": {"msg_id": "execution"}, "content": {"status": "ok"}}


class OutputPolicy(unittest.TestCase):
    def test_updates_replace_and_clear_reclaims_retained_bytes(self):
        budget = bridge["OutputBudget"](limits)
        image = {"output_type": "display_data", "data": {"image/png": "a" * 1200}}
        self.assertTrue(budget.accept(image, "plot", False))
        for _ in range(20):
            self.assertTrue(budget.accept(image, "plot", True))
        before = list(budget.entries)
        self.assertFalse(budget.accept(image, "other", False))
        self.assertEqual(budget.entries, before)
        self.assertFalse(budget.accept({"text": "x" * 300}, None, False, clear=True))
        self.assertEqual(budget.entries, before)
        self.assertTrue(budget.accept(image, "other", False, clear=True))
        self.assertEqual(len(budget.entries), 1)
        budget.clear()
        self.assertEqual(budget.entries, [])

    def test_large_unicode_output_fragments_preserve_the_complete_event(self):
        output = {"data": {"image/png": "a" * 1200, "text/plain": "日本語 🦀"}}
        stream = io.StringIO()
        with contextlib.redirect_stdout(stream):
            bridge["emit_output"](limits, id=17, output=output, update=False, display_id="plot")
        chunks = [json.loads(line) for line in stream.getvalue().splitlines()]
        self.assertTrue(all(chunk["kind"] == "output_chunk" for chunk in chunks))
        self.assertTrue(chunks[-1]["last"])
        result = json.loads("".join(chunk["data"] for chunk in chunks))
        self.assertEqual(result["output"], output)

    def test_limit_interrupts_the_cell_without_closing_the_bridge(self):
        client = Client(
            [
                ("stream", {"text": "earlier output"}),
                ("stream", {"text": "x" * 300}),
                ("stream", {"text": "omitted"}),
                ("status", {"execution_state": "idle"}),
            ]
        )
        manager = Manager()
        stream = io.StringIO()
        with contextlib.redirect_stdout(stream):
            bridge["execute"](manager, client, {"id": 17, "code": "print(value)"}, limits)
        events = [json.loads(line) for line in stream.getvalue().splitlines()]
        self.assertEqual(manager.interrupts, 1)
        self.assertFalse(bridge["closed"].is_set())
        self.assertEqual(events[-1]["status"], "output_limit")
        self.assertEqual(sum(event["kind"] == "warning" for event in events), 1)
        output = json.loads(
            "".join(event["data"] for event in events if event["kind"] == "output_chunk")
        )
        self.assertEqual(output["output"]["text"], "earlier output")

    def test_immediate_clear_replaces_a_pending_deferred_clear(self):
        manager = Manager()
        client = Client(
            [
                ("stream", {"text": "old"}),
                ("clear_output", {"wait": True}),
                ("clear_output", {"wait": False}),
                ("stream", {"text": "new"}),
                ("status", {"execution_state": "idle"}),
            ]
        )
        stream = io.StringIO()
        with contextlib.redirect_stdout(stream):
            bridge["execute"](manager, client, {"id": 18, "code": "clear_output()"}, limits)
        events = [json.loads(line) for line in stream.getvalue().splitlines()]
        self.assertEqual(sum(event["kind"] == "clear" for event in events), 1)
        self.assertEqual(events[-1]["status"], "ok")
        self.assertEqual(manager.interrupts, 0)


unittest.main(argv=[sys.argv[0]])
