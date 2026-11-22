"""Exercise the pinned native broker with synthetic HTTP responses."""

from datetime import datetime, timezone
import hashlib
import json
import os
import time
import unittest
from unittest.mock import patch

import live_provider as provider


WORKSPACE = "wrkspc_test"
KEY = "apikey_test"
HEADERS = {"request-id": "req_test", "anthropic-workspace-id": WORKSPACE}
ENV = {"ANTHROPIC_WORKSPACE_ID": WORKSPACE, "ANTHROPIC_API_KEY_ID": KEY,
       "ANTHROPIC_API_KEY": "fixture-serving", "ANTHROPIC_ADMIN_API_KEY": "fixture-admin"}


def native_reply():
    """Return a paid reply whose adaptive thinking stays out of candidate text."""
    return {
        "id": "msg_test", "model": "claude-sonnet-5", "type": "message", "role": "assistant",
        "stop_reason": "end_turn", "content": [
            {"type": "thinking", "thinking": "private"}, {"type": "text", "text": "{}"}],
        "usage": {"input_tokens": 20, "output_tokens": 5, "inference_geo": "global",
                  "service_tier": "standard", "speed": "standard", "cache_creation_input_tokens": 0,
                  "cache_read_input_tokens": 0, "cache_creation": {"ephemeral_1h_input_tokens": 0,
                    "ephemeral_5m_input_tokens": 0}, "server_tool_use": {"web_search_requests": 0},
                  "output_tokens_details": {"thinking_tokens": 2}}
    }


class ProviderTests(unittest.TestCase):
    """Check effects at the count, reservation, Messages and settlement boundaries."""

    def setUp(self):
        """Provide synthetic identities and one short monotonic deadline."""
        env = patch.object(os, "environ", ENV.copy())
        env.start()
        self.addCleanup(env.stop)
        self.deadline = time.monotonic() + 10
        self.activated = provider.serving_credential_fingerprint()

    def test_free_count_paid_request_and_settlement(self):
        """Reserve before the paid call and keep native usage separate from a bill."""
        public = {"task": "repair", "model": "data only", "tools": ["inert"]}
        with patch.object(provider, "_request", side_effect=[
                (200, HEADERS, b'{"input_tokens":20}'), (200, HEADERS, json.dumps(native_reply()).encode())
        ]) as transport:
            wire, reserved = provider.prepare_message(public, 12000, self.deadline, expected_fingerprint=self.activated)
            self.assertEqual(transport.call_count, 1)
            self.assertEqual(transport.call_args.args[:2], ("POST", "/v1/messages/count_tokens"))
            payload = json.loads(wire)
            self.assertEqual(json.loads(payload["messages"][0]["content"]), public)
            self.assertEqual(payload["inference_geo"], "global")
            self.assertEqual(payload["service_tier"], "standard_only")
            self.assertNotIn("tools", payload)
            self.assertGreaterEqual(reserved["input_tokens"], len(wire))
            self.assertEqual(reserved["total_tokens"], reserved["input_tokens"] + payload["max_tokens"])
            status, headers, body = provider.send_message(wire, self.deadline, reservation=reserved)
            reply, usage = provider.parse_response(status, headers, body, reserved)
            self.assertEqual(transport.call_args.args[:3], ("POST", "/v1/messages", wire))
            self.assertEqual((reply, usage["list_microusd"], usage["outcome"]), (b"{}", 90, "complete"))
            self.assertNotIn("billed_microusd", usage)
            self.assertEqual(reserved["wire_sha256"], hashlib.sha256(wire).hexdigest())
            with patch.dict(os.environ, {"ANTHROPIC_API_KEY_ID": "apikey_other"}), self.assertRaises(provider.ProviderError):
                provider.send_message(wire, self.deadline, reservation=reserved)
            self.assertEqual(transport.call_count, 2)

    def test_resampling_wire_stays_identical_and_limits_precede_paid_effect(self):
        """A repeat uses identical output settings even with less capacity left."""
        with patch.object(provider, "_request", return_value=(200, HEADERS, b'{"input_tokens":50}')) as native:
            first, reserved = provider.prepare_message({"task": "repeat"}, 12000, self.deadline,
                                                       expected_fingerprint=self.activated)
            second, _ = provider.prepare_message({"task": "repeat"}, 8000, self.deadline,
                                                 expected_fingerprint=self.activated)
            self.assertEqual(first, second)
            with self.assertRaises(provider.ReservationError):
                provider.prepare_message({"task": "x" * 2048}, 3000, self.deadline, expected_fingerprint=self.activated)
            native.return_value = (200, HEADERS, b'{"input_tokens":12000}')
            with self.assertRaises(provider.ReservationError):
                provider.prepare_message({"task": "repeat"}, 12000, self.deadline, expected_fingerprint=self.activated)
            self.assertEqual([call.args[1] for call in native.call_args_list],
                             ["/v1/messages/count_tokens"] * 3)
            native.reset_mock()
            with self.assertRaises(provider.ProviderError):
                provider.send_message(first.replace(b"standard_only", b"auto"), self.deadline, reservation=reserved)
            native.assert_not_called()

    def test_credentials_and_response_binding(self):
        """A missing key prevents even the free control, and a foreign count holds the run."""
        with patch.object(provider, "_request") as native:
            with patch.dict(os.environ, {"ANTHROPIC_API_KEY": ""}):
                with self.assertRaisesRegex(provider.ProviderError, "ANTHROPIC_API_KEY"):
                    provider.prepare_message({}, 12000, self.deadline, expected_fingerprint=self.activated)
                native.assert_not_called()
            native.return_value = (200, {**HEADERS, "anthropic-workspace-id": "wrkspc_other"},
                                   b'{"input_tokens":2}')
            with self.assertRaisesRegex(provider.ProviderError, "workspace"):
                provider.prepare_message({}, 12000, self.deadline, expected_fingerprint=self.activated)

    def test_swapped_serving_credential_stops_paid_dispatch(self):
        """Keep a replacement credential from serving a request reserved under the original key."""
        with patch.object(provider.http.client, "HTTPSConnection") as constructor:
            connection = constructor.return_value
            response = connection.getresponse.return_value
            response.status, response.length = 200, 0
            response.getheader.side_effect = HEADERS.get
            response.read1.side_effect = [b'{"input_tokens":20}', b"", b"{}", b""]
            wire, reservation = provider.prepare_message({}, 12000, self.deadline, expected_fingerprint=self.activated)
            self.assertEqual(connection.request.call_count, 1)
            with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "fixture-replacement"}):
                with self.assertRaisesRegex(provider.ProviderError, "credential"):
                    provider.send_message(wire, self.deadline, reservation=reservation)
            connection.request.assert_called_once()
            self.assertNotIn(ENV["ANTHROPIC_API_KEY"], json.dumps(reservation))

    def test_credential_swap_before_count_and_during_header_construction(self):
        """Refuse a changed activation binding and use each checked snapshot during transport."""
        actual_request = provider._request

        def swap_before_headers(*args, **kwargs):
            """Rotate the synthetic environment after the check and before header construction."""
            with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "fixture-replacement"}):
                return actual_request(*args, **kwargs)

        with patch.object(provider.http.client, "HTTPSConnection") as constructor, \
                patch.object(provider, "_request", side_effect=swap_before_headers):
            connection = constructor.return_value
            response = connection.getresponse.return_value
            response.status, response.length = 200, 0
            response.getheader.side_effect = HEADERS.get
            response.read1.side_effect = [b'{"input_tokens":20}', b"", b"{}", b""]
            for binding in ({"ANTHROPIC_API_KEY": "fixture-other-activation"},
                            {"ANTHROPIC_WORKSPACE_ID": "wrkspc_other"}, {"ANTHROPIC_API_KEY_ID": "apikey_other"}):
                with self.subTest(binding=list(binding)), patch.dict(os.environ, binding):
                    with self.assertRaisesRegex(provider.ProviderError, "credential"):
                        provider.prepare_message({}, 12000, self.deadline, expected_fingerprint=self.activated)
            constructor.assert_not_called()
            wire, reservation = provider.prepare_message({}, 12000, self.deadline,
                                                         expected_fingerprint=self.activated)
            provider.send_message(wire, self.deadline, reservation=reservation)
            self.assertEqual(reservation["credential_fingerprint"], self.activated)
            self.assertRegex(self.activated, r"^[0-9a-f]{64}$")
            self.assertEqual([call.kwargs["headers"]["x-api-key"] for call in connection.request.call_args_list],
                             [ENV["ANTHROPIC_API_KEY"]] * 2)

    def test_ambiguous_identity_usage_or_service_holds(self):
        """Reject missing usage, cost drift, foreign scopes and invalid native replies."""
        reserved = {"workspace_id": WORKSPACE, "input_tokens": 30, "output_tokens": 10}
        for field, value in [("input_tokens", True), ("output_tokens", -1), ("output_tokens", 11),
                             ("input_tokens", 31), ("service_tier", "priority"), ("speed", "fast"),
                             ("inference_geo", "us"), ("cache_read_input_tokens", 1),
                             ("server_tool_use", {"web_search_requests": 1}), ("unknown_charge", 1)]:
            data = native_reply()
            data["usage"][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(provider.ProviderError):
                provider.parse_response(200, HEADERS, json.dumps(data).encode(), reserved)
        for change in ({"model": "unapproved"}, {"usage": None}, {"stop_reason": "tool_use"}):
            with self.subTest(change=change), self.assertRaises(provider.ProviderError):
                provider.parse_response(200, HEADERS, json.dumps({**native_reply(), **change}).encode(), reserved)
        for status, headers, body in [
                (307, HEADERS, b"redirect"), (500, HEADERS, b"service error"),
                (200, {"request-id": "req_test"}, json.dumps(native_reply()).encode()),
                (200, HEADERS, b'{"id":1,"id":2}'), (200, HEADERS, b"broken")]:
            with self.subTest(status=status, headers=headers), self.assertRaises(provider.ProviderError):
                provider.parse_response(status, headers, body, reserved)

    def test_authoritative_model_stop_keeps_usage(self):
        """A refusal and a truncated reply retain their exact charges and stop reasons."""
        for reason, outcome in (("refusal", "refused"), ("max_tokens", "incomplete"),
                                ("model_context_window_exceeded", "incomplete")):
            data = {**native_reply(), "stop_reason": reason}
            data["usage"]["server_tool_use"] = None
            reserved = {"workspace_id": WORKSPACE, "input_tokens": 25, "output_tokens": 10}
            _, usage = provider.parse_response(200, HEADERS, json.dumps(data).encode(), reserved)
            self.assertEqual((usage["stop_reason"], usage["outcome"], usage["list_microusd"]),
                             (reason, outcome, 90))

    def test_direct_transport_selects_one_role_and_never_follows_redirect(self):
        """Proxy and SDK settings can't change the endpoint or replay a redirect."""
        with patch.dict(os.environ, {"HTTPS_PROXY": "http://invalid.example",
                                     "ANTHROPIC_BASE_URL": "https://invalid.example"}), \
                patch.object(provider.http.client, "HTTPSConnection") as constructor:
            connection = constructor.return_value
            response = connection.getresponse.return_value
            response.status, response.length = 307, 0
            response.getheader.side_effect = HEADERS.get
            response.read1.side_effect = [b"redirect body", b""]
            status, headers, raw = provider._request("POST", "/v1/messages", b"{}", self.deadline,
                                                     workspace_id=WORKSPACE, serving_key=ENV["ANTHROPIC_API_KEY"])
            self.assertEqual((status, headers, raw), (307, HEADERS, b"redirect body"))
            args, kwargs = constructor.call_args
            self.assertEqual(args, ("api.anthropic.com",))
            self.assertTrue(kwargs["context"].check_hostname)
            self.assertEqual(kwargs["context"].verify_mode, provider.ssl.CERT_REQUIRED)
            sent = connection.request.call_args.kwargs["headers"]
            self.assertEqual((sent["x-api-key"], sent["anthropic-workspace-id"]), ("fixture-serving", WORKSPACE))
            connection.request.assert_called_once()
            connection.close.assert_called_once()
            connection.reset_mock()
            response.read1.side_effect = [b"admin body", b""]
            provider._request("GET", "/v1/organizations/analytics/usage", None, self.deadline, admin=True)
            sent = connection.request.call_args.kwargs["headers"]
            self.assertEqual(sent["x-api-key"], "fixture-admin")
            self.assertNotIn("anthropic-workspace-id", sent)
            self.assertEqual(sent["anthropic-beta"], provider.ANALYTICS_BETA)

    def test_transport_preserves_holds_on_timeout_oversize_or_truncation(self):
        """Keep interrupted, truncated and oversized native reads unsettled without replay."""
        with patch.object(provider.http.client, "HTTPSConnection") as constructor:
            for raw, pending in [(b"x" * (provider.MAX_BODY + 1), 0), (b"truncated", 1)]:
                connection = constructor.return_value
                response = connection.getresponse.return_value
                response.status, response.length = 200, pending
                response.getheader.side_effect = HEADERS.get
                response.read1.side_effect = [raw, b""]
                status, headers, observed = provider._request("POST", "/v1/messages", b"{}", self.deadline,
                                                             workspace_id=WORKSPACE, serving_key=ENV["ANTHROPIC_API_KEY"])
                self.assertEqual(observed, raw[:provider.MAX_BODY])
                self.assertIn(headers["body_status"], ("oversized", "incomplete"))
                with self.assertRaises(provider.ProviderError):
                    provider.parse_response(status, headers, observed, {"workspace_id": WORKSPACE})
                connection.request.assert_called_once()
                connection.close.assert_called_once()
                connection.reset_mock()
            connection.getresponse.side_effect = TimeoutError()
            with self.assertRaises(provider.ProviderError):
                provider._request("POST", "/v1/messages", b"{}", self.deadline,
                                  workspace_id=WORKSPACE, serving_key=ENV["ANTHROPIC_API_KEY"])
            connection.request.assert_called_once()
            connection.reset_mock()
            with self.assertRaises(provider.ProviderError):
                provider._request("POST", "/v1/messages", b"{}", time.monotonic() - 1,
                                  workspace_id=WORKSPACE, serving_key=ENV["ANTHROPIC_API_KEY"])
            connection.request.assert_not_called()


class BillingTests(unittest.TestCase):
    """Reconcile separately collected admin pages against known private ledger totals."""

    def setUp(self):
        """Make a two-minute exclusive window containing one request in each minute."""
        self.start = datetime(2026, 11, 20, 9, 10, 59, tzinfo=timezone.utc)
        self.stop = datetime(2026, 11, 20, 9, 11, 30, tzinfo=timezone.utc)
        self.args = (WORKSPACE, KEY, self.start, self.stop, time.monotonic() + 5)
        self.expected = {"requests": 2, "input_tokens": 40, "output_tokens": 10, "list_microusd": 180}

    def page(self, kind, minute, follows=False):
        """Build one documented analytics bucket with a sub-microdollar discounted cost."""
        group = {"workspace_id": WORKSPACE, "api_key_id": KEY, "model": "claude-sonnet-5",
                 "service_tier": "standard", "inference_geo": "global"}
        usage = {"uncached_input_tokens": 20, "output_tokens": 5, "total_tokens": 25,
                 "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0},
                 "cache_read_input_tokens": 0, "server_tool_use": {"web_search_requests": 0}, "requests": 1}
        cost = {"list_amount": "0.009000", "amount": "0.008401", "requests": 1, "currency": "USD"}
        row = {"group": group, "starting_at": f"2026-11-20T09:{minute:02d}:00Z",
               "ending_at": f"2026-11-20T09:{minute + 1:02d}:00Z", kind: usage if kind == "usage" else cost}
        return {"data": [row], "has_more": follows, "next_page": "page_2" if follows else None,
                "data_refreshed_at": "2026-11-21T04:00:00Z"}

    def collect(self, pages=None):
        """Collect every native page through the public admin path without contacting the service."""
        if pages is None:
            pages = [self.page(kind, minute, minute == 10) for kind in ("usage", "cost") for minute in (10, 11)]
        responses = [(200, {"request-id": "req_" + str(index)}, json.dumps(page).encode())
                     for index, page in enumerate(pages)]
        with patch.object(provider, "_request", side_effect=responses) as native:
            capture = provider.capture_billing(*self.args)
            self.assertTrue(all(call.kwargs.get("admin") is True for call in native.call_args_list))
            self.assertTrue(all(call.args[0] == "GET" for call in native.call_args_list))
            self.assertTrue(all("workspace_ids%5B%5D=wrkspc_test" in call.args[1] and
                                "api_key_ids%5B%5D=apikey_test" in call.args[1] for call in native.call_args_list))
            self.assertIn("page=page_2", native.call_args_list[1].args[1])
        return capture

    def replay(self, capture):
        """Authenticate the administrator's independently retained digest and recheck every page."""
        return provider.fetch_billing(*self.args, capture=capture, trusted_sha256=hashlib.sha256(capture).hexdigest())

    def test_native_pagination_and_exact_discounted_cents(self):
        """Match both native counts and signed-off list totals while preserving fractional charges."""
        capture = self.collect()
        with patch.object(provider, "_request", side_effect=AssertionError("offline replay")):
            report = self.replay(capture)
            provider.reconcile_billing(report, self.expected)
            self.assertEqual(report["billed_cents"], "0.016802")
            self.assertIsNone(report["billed_microusd"])
            self.assertFalse(report["all_rows_at_list"])
            self.assertEqual(report["capture"]["scope"]["ending_at"], "2026-11-20T09:12:00Z")
            for key in self.expected:
                with self.subTest(key=key), self.assertRaises(provider.ProviderError):
                    provider.reconcile_billing(report, {**self.expected, key: self.expected[key] + 1})
            with self.assertRaises(provider.ProviderError):
                provider.fetch_billing(*self.args, capture=capture, trusted_sha256="0"*64)

    def test_undiscounted_minutes_match_the_full_local_receipt_total(self):
        """Return whole microdollars when each native cost row equals the observed list price."""
        packet = json.loads(self.collect())
        for page in packet["pages"]["cost"]:
            native = json.loads(page["body"])
            cost = native["data"][0]["cost"]
            cost["amount"] = cost["list_amount"]
            page["body"] = json.dumps(native)
        report = self.replay(json.dumps(packet).encode())
        provider.reconcile_billing(report, self.expected)
        self.assertTrue(report["all_rows_at_list"])
        self.assertEqual((report["billed_cents"], report["billed_microusd"]), ("0.018000", 180))

    def test_missing_stale_mixed_or_foreign_admin_capture(self):
        """Require all pages, the requested scope, closed coverage and a stable native snapshot."""
        packet = json.loads(self.collect())
        for fault in ("missing", "extra", "path", "watermark", "unrefreshed", "foreign", "bucket",
                      "duplicate", "unaccounted", "cost_count", "rate", "precision", "currency"):
            altered = json.loads(json.dumps(packet))
            pages = altered["pages"]["usage"]
            kind = "usage"
            if fault in ("cost_count", "rate", "precision", "currency"):
                kind = "cost"
                pages = altered["pages"]["cost"]
            entry = pages[-1]
            body = json.loads(entry["body"])
            row = body["data"][0]
            if fault == "missing":
                pages.pop()
            elif fault == "extra":
                pages.append(entry.copy())
            elif fault == "path":
                entry["path"] = entry["path"].replace(KEY, "apikey_foreign")
            elif fault == "watermark":
                body["data_refreshed_at"] = "2026-11-21T04:00:01Z"
            elif fault == "unrefreshed":
                body["data_refreshed_at"] = "2026-11-20T09:11:00Z"
            elif fault == "foreign":
                row["group"]["workspace_id"] = "wrkspc_foreign"
            elif fault == "bucket":
                row["ending_at"] = "2026-11-20T09:13:00Z"
            elif fault == "duplicate":
                body["data"].append(row)
            elif fault == "unaccounted":
                row[kind]["total_tokens"] += 1
            elif fault == "cost_count":
                row[kind]["requests"] = 0
            elif fault == "rate":
                row[kind]["list_amount"] = "0.010000"
            elif fault == "precision":
                row[kind]["amount"] = "0.0000001"
            elif fault == "currency":
                row[kind]["currency"] = "EUR"
            entry["body"] = json.dumps(body)
            with self.subTest(fault=fault), self.assertRaises(provider.ProviderError):
                self.replay(json.dumps(altered).encode())

    def test_minute_adjustments_cannot_offset(self):
        """Reject offsetting price and count changes even when the run-wide sums agree."""
        capture = self.collect()
        for field, values in (("amount", ["0.010000", "0.006802"]), ("requests", [2, 0]),
                              ("list_amount", ["0.010000", "0.008000"])):
            packet = json.loads(capture)
            for page, value in zip(packet["pages"]["cost"], values):
                native = json.loads(page["body"])
                native["data"][0]["cost"][field] = value
                page["body"] = json.dumps(native)
            with self.subTest(field=field), self.assertRaises(provider.ProviderError):
                self.replay(json.dumps(packet).encode())

    def test_repeated_cursor_cannot_trigger_replay(self):
        """Stop at the second repeated cursor while reading the admin endpoint."""
        repeated = self.page("usage", 10, True)
        with patch.object(provider, "_request", return_value=(200, HEADERS, json.dumps(repeated).encode())) as native:
            with self.assertRaises(provider.ProviderError):
                provider.capture_billing(*self.args)
            self.assertEqual(native.call_count, 2)


if __name__ == "__main__":
    unittest.main()
