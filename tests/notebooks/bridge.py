"""Exercise bridge accounting without requiring an installed Jupyter kernel."""

import contextlib
import io
import json
import sys
import unittest

bridge = {"__name__": "rode_notebook_bridge_test"}
exec(compile(sys.argv[1], "notebook_kernel.py", "exec"), bridge)
limits = {"bytes": 2048, "overhead": 128, "chunk_chars": 32, "message": 4096}


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
    def test_ram_allowance_is_captured_per_execution_and_rejects_invalid_requests(self):
        policy = dict(limits, bytes=2 * 1024 * 1024, message=2 * 1024 * 1024, chunk_chars=65536)
        for allowance, warning in [(1024 * 1024, True), (2 * 1024 * 1024, False)]:
            manager = Manager()
            client = Client(
                [("stream", {"text": "x" * (1536 * 1024)}), ("status", {"execution_state": "idle"})]
            )
            stream = io.StringIO()
            with contextlib.redirect_stdout(stream):
                bridge["execute"](
                    manager,
                    client,
                    {"id": 17, "code": "print(value)", "output_budget": allowance},
                    policy,
                )
            events = [json.loads(line) for line in stream.getvalue().splitlines()]
            self.assertEqual(any(event["kind"] == "warning" for event in events), warning)
            self.assertEqual(events[-1]["status"], "ok")
            self.assertEqual(manager.interrupts, 0)
        for invalid in [False, 0, -1, 4 * 1024 * 1024]:
            with self.assertRaises(ValueError):
                bridge["execute"](
                    Manager(), Client([]), {"id": 17, "code": "", "output_budget": invalid}, policy
                )

    def test_stream_fragments_share_one_record_and_match_json_byte_accounting(self):
        budget = bridge["OutputBudget"](dict(limits, bytes=1024 * 1024))
        joined = ""
        for fragment in ["日", "🦀", "\n", '"', "\\"] * 70:
            output = dict(output_type="stream", name="stdout", text=fragment)
            self.assertTrue(budget.accept(output, None, False), budget.reason)
            joined += fragment
        retained = dict(output_type="stream", name="stdout", text=joined)
        size = len(bridge["compact"](retained).encode("utf-8"))
        self.assertEqual(len(budget.entries), 1)
        self.assertEqual(budget.entries[0][0], size + limits["overhead"])
        for output in [
            dict(output_type="stream", name="stderr", text="error"),
            dict(output_type="display_data", data={"text/plain": "result"}),
            dict(output_type="stream", name="stdout", text="after display"),
        ]:
            self.assertTrue(budget.accept(output, None, False), budget.reason)
        # Record count is not a separate retention limit.
        for _ in range(350):
            self.assertTrue(
                budget.accept(
                    dict(output_type="display_data", data={"text/plain": "small"}), None, False
                )
            )
        self.assertEqual(len(budget.entries), 354)

    def test_updates_replace_and_clear_reclaims_retained_bytes(self):
        budget = bridge["OutputBudget"](limits)
        image = {"output_type": "display_data", "data": {"image/png": "a" * 1200}}
        self.assertTrue(budget.accept(image, "plot", False))
        for _ in range(20):
            self.assertTrue(budget.accept(image, "plot", True))
        before = list(budget.entries)
        self.assertFalse(budget.accept(image, "other", False))
        self.assertEqual(budget.entries, before)
        self.assertFalse(budget.accept({"text": "x" * 3000}, None, False, clear=True))
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

    def test_limits_omit_only_rejected_output_without_interrupting_the_cell(self):
        client = Client(
            [
                ("stream", {"text": "earlier output"}),
                ("stream", {"text": "x" * 3000}),
                ("stream", {"text": "later output"}),
                ("stream", {"text": "x" * 4000}),
                ("clear_output", {"wait": True}),
                ("stream", {"text": "x" * 3000}),
                ("display_data", {"data": {"image/png": "a" * 1200}}),
                ("status", {"execution_state": "idle"}),
            ]
        )
        manager = Manager()
        stream = io.StringIO()
        with contextlib.redirect_stdout(stream):
            bridge["execute"](manager, client, {"id": 17, "code": "print(value)"}, limits)
        events = [json.loads(line) for line in stream.getvalue().splitlines()]
        self.assertEqual(manager.interrupts, 0)
        self.assertFalse(bridge["closed"].is_set())
        self.assertEqual(events[-1]["status"], "ok")
        self.assertEqual(sum(event["kind"] == "warning" for event in events), 1)
        self.assertEqual(sum(event["kind"] == "clear" for event in events), 1)
        outputs, pending = [], ""
        for event in events:
            if event["kind"] == "output":
                outputs.append(event["output"])
            elif event["kind"] == "output_chunk":
                pending += event["data"]
                if event["last"]:
                    outputs.append(json.loads(pending)["output"])
                    pending = ""
        self.assertEqual(outputs[0]["text"], "earlier output")
        self.assertEqual(outputs[1]["text"], "later output")
        self.assertEqual(outputs[2]["data"]["image/png"], "a" * 1200)
        warning = next(event["message"] for event in events if event["kind"] == "warning")
        self.assertIn("output RAM allowance", warning)
        self.assertIn("Computation continues", warning)

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
