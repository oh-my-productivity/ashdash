"""Tests of typing_stats.py: python3 -m unittest scripts/qa/typing_stats_test.py (no desktop needed)."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import typing_stats as ts  # noqa: E402

TRACE = [
    "[trace      12ms] focus=Some(true) +3ms after show",
    '[ft] t=1000.0 request gen=1 q="p"',
    '[ft] t=1012.5 frame dur=2.10 gap=16.7 text=1 q="p" gen=1 rows=0 logic{poll_background=0.10} ui{keys=0.20}',
    '[ft] t=1100.0 request gen=2 q="pu"',
    '[ft] t=1180.0 frame dur=40.00 gap=20.0 text=1 q="pu" gen=2 rows=0 SLOW logic{} ui{results=39.00}',
    '[ft] t=1190.0 results gen=1 kind=Final rows=9',
    '[ft] t=1300.0 request gen=3 q="pup"',
    '[ft] t=1310.0 frame dur=1.00 gap=16.7 text=1 q="pup" gen=3 rows=9 logic{} ui{}',
    '[ft] t=1250.0 results gen=2 kind=Pending rows=1',
    '[ft] t=1400.0 results gen=3 kind=Final rows=9',
    '[ft] t=1410.0 results gen=2 kind=Final rows=9',
]


class ParseTest(unittest.TestCase):
    def test_other_lines_are_ignored_and_fields_are_read(self):
        ev = ts.parse_trace(TRACE)
        self.assertEqual(len(ev), 10)
        frame = ev[1]
        self.assertEqual((frame["type"], frame["t"], frame["q"], frame["gen"], frame["rows"]), ("frame", 1012.5, "p", 1, 0))
        self.assertEqual((frame["dur"], frame["gap"], frame["text"], frame["slow"]), (2.1, 16.7, 1, False))
        self.assertTrue(ev[3]["slow"])
        self.assertEqual(ev[4]["kind"], "Final")

    def test_a_query_with_quotes_survives(self):
        ev = ts.parse_trace(['[ft] t=5.0 request gen=1 q="say \\"hi\\""'])
        self.assertEqual(ev[0]["q"], 'say "hi"')


class LatencyTest(unittest.TestCase):
    def setUp(self):
        self.events = ts.parse_trace(TRACE)
        self.keys = [(0, "p", 1000.0), (1, "u", 1100.0), (2, "p", 1300.0)]

    def test_glyph_latency_is_to_the_first_frame_that_shows_the_prefix(self):
        rows = ts.key_latencies(self.keys, self.events, "pup")
        self.assertEqual([r["glyph_ms"] for r in rows], [12.5, 80.0, 10.0])

    def test_results_latency_is_to_the_final_answer_for_that_exact_prefix(self):
        rows = ts.key_latencies(self.keys, self.events, "pup")
        # gen 1 is "p": answered at 1190 (190 ms after its key); gen 2 "pu": the Pending batch does not count, Final at 1410
        self.assertEqual([r["results_ms"] for r in rows], [190.0, 310.0, 100.0])

    def test_missing_data_is_none_not_zero(self):
        rows = ts.key_latencies([(0, "p", 5000.0)], self.events, "pup")
        self.assertEqual((rows[0]["glyph_ms"], rows[0]["results_ms"]), (None, None))

    def test_a_frame_before_the_key_does_not_count(self):
        ev = ts.parse_trace(['[ft] t=900.0 frame dur=1.0 gap=1.0 text=1 q="p" gen=1 rows=0 logic{} ui{}'])
        self.assertIsNone(ts.key_latencies([(0, "p", 1000.0)], ev, "p")[0]["glyph_ms"])


class StatsTest(unittest.TestCase):
    def test_percentiles_use_the_nearest_rank(self):
        self.assertEqual(ts.percentile([1, 2, 3, 4], 50), 2)
        self.assertEqual(ts.percentile([1, 2, 3, 4], 95), 4)
        self.assertEqual(ts.percentile(list(range(1, 101)), 95), 95)
        self.assertEqual(ts.percentile([7], 50), 7)
        self.assertIsNone(ts.percentile([], 50))
        self.assertIsNone(ts.percentile([None], 50))

    def test_summary_counts_missing_values(self):
        rows = [
            {"index": 0, "char": "p", "glyph_ms": 10.0, "results_ms": None},
            {"index": 0, "char": "p", "glyph_ms": 30.0, "results_ms": 5.0},
            {"index": 1, "char": "u", "glyph_ms": 20.0, "results_ms": 7.0},
        ]
        s = ts.summarize(rows)
        self.assertEqual(s["all"]["glyph_ms"], {"n": 3, "missing": 0, "p50": 20.0, "p95": 30.0, "max": 30.0})
        self.assertEqual(s["all"]["results_ms"]["missing"], 1)
        self.assertEqual(s["by_index"][0]["glyph_ms"]["p50"], 10.0)
        self.assertEqual(sorted(s["by_index"]), [0, 1])

    def test_the_slowest_frames_come_first(self):
        ev = ts.parse_trace(TRACE)
        lines = ts.slow_frames(ev, since_ms=0.0, limit=2)
        self.assertEqual(len(lines), 2)
        self.assertIn("dur=40.00", lines[0])
        self.assertEqual(len(ts.slow_frames(ev, since_ms=1200.0)), 1)


if __name__ == "__main__":
    unittest.main()


class ReportTest(unittest.TestCase):
    def test_the_report_names_typed_frames_answers_and_slow_frames(self):
        text = ts.report(ts.parse_trace(TRACE))
        self.assertIn("3 frames drew typed text, 3 queries were requested", text)
        # "pu" arrived 310 ms after its request (the Pending batch does not count) and its frame was slow
        self.assertIn("'pu'", text)
        self.assertIn("answer 310.0 ms", text)
        self.assertIn("1 slow frames", text)
        self.assertIn("ui{results=39.00}", text)

    def test_a_long_gap_is_marked(self):
        ev = ts.parse_trace(['[ft] t=5.0 frame dur=0.5 gap=250.0 text=1 q="p" gen=1 rows=0 logic{} ui{}'])
        self.assertIn("<-- long gap", ts.report(ev))
        self.assertIn("no answer traced", ts.report(ev))


class LimitsTest(unittest.TestCase):
    @staticmethod
    def rows(glyph, results):
        return [{"index": i, "char": "p", "glyph_ms": g, "results_ms": r} for i, (g, r) in enumerate(zip(glyph, results))]

    def test_a_fast_run_passes(self):
        s = ts.summarize(self.rows([5.0, 12.0, 20.0], [20.0, 30.0, 40.0]))
        self.assertEqual(ts.check_limits(s, "heavy"), [])

    def test_a_slow_glyph_or_answer_fails_with_a_sentence(self):
        s = ts.summarize(self.rows([5.0, 12.0, 80.0], [20.0, 30.0, 400.0]))
        out = ts.check_limits(s, "heavy")
        self.assertEqual(len(out), 2, out)
        self.assertIn("key-to-glyph p95 80.0 ms is over 50.0 ms", out[0])
        self.assertIn("key-to-results p95 400.0 ms is over 150.0 ms", out[1])
        # the slow variant is allowed longer for its answers, never for the typed character
        self.assertEqual(len(ts.check_limits(s, "heavy-slow")), 1)

    def test_nothing_measured_is_a_failure_not_a_pass(self):
        s = ts.summarize(self.rows([None, None, None], [None, None, None]))
        out = ts.check_limits(s, "heavy")
        self.assertEqual(len(out), 2, out)
        self.assertTrue(all("no key-to" in x for x in out))
        self.assertEqual(ts.check_limits(ts.summarize([]), "heavy-slow")[0], "heavy-slow: no key-to-glyph latency was measured")


class TaintedKeysTest(unittest.TestCase):
    def test_a_key_the_driver_handed_over_late_is_counted_and_not_measured(self):
        ev = ts.parse_trace(['[ft] t=1900.0 frame dur=0.5 gap=1.0 text=1 q="p" gen=1 rows=0 logic{} ui{}',
                             '[ft] t=1960.0 frame dur=0.5 gap=1.0 text=1 q="pu" gen=2 rows=0 logic{} ui{}'])
        rows = ts.key_latencies([(0, "p", 1000.0, 850.0), (1, "u", 1950.0, 4.0)], ev, "pu")
        self.assertEqual([r["post_ms"] for r in rows], [850.0, 4.0])
        s = ts.summarize(rows)
        self.assertEqual(s["tainted"], 1)
        self.assertEqual((s["all"]["glyph_ms"]["n"], s["all"]["glyph_ms"]["max"]), (1, 10.0))
        self.assertEqual(ts.summarize([])["tainted"], 0)

    def test_a_run_whose_driver_stalled_all_the_time_fails(self):
        rows = [{"index": 0, "char": "p", "glyph_ms": 5.0, "results_ms": 9.0, "post_ms": 500.0}] * 5 + [
            {"index": 0, "char": "p", "glyph_ms": 5.0, "results_ms": 9.0, "post_ms": 3.0}
        ]
        out = ts.check_limits(ts.summarize(rows), "heavy")
        self.assertEqual(len(out), 1, out)
        self.assertIn("stalled on 5 of 6 keys", out[0])


EVENT_TRACE = [
    '[ft] t=1000.0 event key down code=35 age=2.0 slept_before=400.0 awake_for=0.1 prev_busy=0.4',
    '[ft] t=1003.0 frame dur=0.2 gap=400.0 text=1 q="p" gen=1 rows=0 logic{} ui{}',
    '[ft] t=1100.0 event key up code=35 age=1.0 slept_before=0.5 awake_for=0.1 prev_busy=0.4',
    '[ft] t=2100.0 loop busy=1003.1',
    '[ft] t=2101.0 event key down code=35 age=1001.5 slept_before=0.2 awake_for=0.1 prev_busy=1003.1',
    '[ft] t=2102.0 event key down code=35 age=950.0 slept_before=0.2 awake_for=0.5 prev_busy=1003.1',
    '[ft] t=2105.5 frame dur=0.2 gap=1103.0 text=2 q="ppp" gen=3 rows=0 logic{} ui{}',
]


class EventTraceTest(unittest.TestCase):
    def test_event_and_loop_lines_are_parsed(self):
        ev = ts.parse_trace(EVENT_TRACE)
        self.assertEqual([e["type"] for e in ev], ["event", "frame", "event", "loop", "event", "event", "frame"])
        self.assertEqual((ev[0]["key"], ev[0]["age"], ev[0]["slept_before"], ev[0]["code"]), ("down", 2.0, 400.0, 35))
        self.assertEqual(ev[3]["busy"], 1003.1)

    def test_each_key_down_gets_the_frame_that_drew_it(self):
        d = ts.event_delays(ts.parse_trace(EVENT_TRACE))
        self.assertEqual([x["to_frame"] for x in d], [3.0, 4.5, 3.5])
        self.assertEqual([x["age"] for x in d], [2.0, 1001.5, 950.0])

    def test_a_key_that_waited_while_the_thread_was_busy_says_blocked(self):
        v = ts.verdict(ts.parse_trace(EVENT_TRACE))
        self.assertIn("2 of 3 keys waited", v)
        self.assertIn("main thread was blocked", v)

    def test_a_key_that_waited_while_the_loop_slept_says_woken_late(self):
        v = ts.verdict(ts.parse_trace(["[ft] t=1.0 event key down code=1 age=900.0 slept_before=900.0 awake_for=0.1 prev_busy=0.1"]))
        self.assertIn("loop slept and was woken late", v)

    def test_no_late_key_and_no_events(self):
        self.assertIn("no late key", ts.verdict(ts.parse_trace(EVENT_TRACE[:2])))
        self.assertIn("no key events were traced", ts.verdict(ts.parse_trace(TRACE)))

    def test_the_report_lists_late_keys_and_the_verdict(self):
        out = ts.report(ts.parse_trace(EVENT_TRACE))
        self.assertIn("<-- late", out)
        self.assertIn("verdict:", out)
        self.assertIn("busy 1003.1 ms", out)


def idle_row(idle, glyph, post=3.0, index=3):
    return {"index": index, "char": "p", "glyph_ms": glyph, "results_ms": None, "post_ms": post, "idle_s": idle}


class IdleTest(unittest.TestCase):
    def test_summary_is_per_pause_and_leaves_out_late_driver_keys(self):
        rows = [idle_row(1.0, g) for g in (2.0, 3.0, 4.0, 5.0)] + [idle_row(2.0, 900.0, post=500.0), idle_row(2.0, 6.0)]
        s = ts.summarize_idle(rows)
        self.assertEqual(sorted(s), [1.0, 2.0])
        self.assertEqual((s[1.0]["n"], s[1.0]["p50"], s[1.0]["max"], s[1.0]["stalls"]), (4, 3.0, 5.0, 0))
        self.assertEqual((s[2.0]["n"], s[2.0]["tainted"], s[2.0]["max"]), (1, 1, 6.0))
        self.assertEqual(ts.check_idle_limits(s), [])

    def test_a_one_second_key_breaks_the_limit_even_when_p95_is_fine(self):
        rows = [idle_row(1.0, 3.0)] * 99 + [idle_row(1.0, 1004.0)]
        s = ts.summarize_idle(rows)
        self.assertEqual(s[1.0]["p95"], 3.0)
        self.assertEqual(s[1.0]["stalls"], 1)
        out = ts.check_idle_limits(s)
        self.assertEqual(len(out), 1, out)
        self.assertIn("1 of 100 keys were drawn 500 ms or later (max 1004.0 ms)", out[0])

    def test_a_slow_p95_and_no_data_fail(self):
        rows = [idle_row(0.3, 60.0)] * 5
        out = ts.check_idle_limits(ts.summarize_idle(rows))
        self.assertIn("p95 60.0 ms is over 50 ms", out[0])
        self.assertEqual(ts.check_idle_limits({}), ["idle: no key was measured"])
        self.assertIn("no key-to-glyph latency", ts.check_idle_limits(ts.summarize_idle([idle_row(1.0, None)]))[0])


class BatchedKeysTest(unittest.TestCase):
    def test_keys_drawn_by_one_late_frame_all_get_that_frame(self):
        ev = ts.parse_trace(['[ft] t=2005.0 frame dur=0.2 gap=1003.0 text=3 q="puppp" gen=9 rows=0 logic{} ui{}'])
        rows = ts.key_latencies([(2, "p", 1000.0), (3, "p", 1050.0), (4, "p", 1100.0)], ev, "puppp")
        self.assertEqual([r["glyph_ms"] for r in rows], [1005.0, 955.0, 905.0])
