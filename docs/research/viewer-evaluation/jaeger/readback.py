"""Compare recorded GAAP projections with Jaeger's v3 query response."""

import argparse
import json
from contextlib import closing
from datetime import datetime, timedelta, timezone
from http.client import HTTPConnection
from pathlib import Path
from urllib.parse import urlencode


def attributes(record):
    """Index OTLP attributes by key while preserving their typed values."""
    return {item["key"]: item["value"] for item in record.get("attributes", [])}


def check_fields(source, returned):
    """Check that stored attributes retain their values and GAAP correlation keys."""
    actual = attributes(returned)
    expected = attributes(source)
    for key, value in expected.items():
        if actual.get(key) != value:
            raise ValueError(f"Viewer changed {key}")
    for key in actual:
        if key.startswith(("gaap.", "threadloop.")) and key not in expected:
            raise ValueError(f"Viewer added {key}")


def query(path, params=None):
    """Read a JSON response from the loopback Jaeger v3 query endpoint."""
    target = f"/api/v3/{path}"
    if params:
        target += "?" + urlencode(params)
    with closing(HTTPConnection("127.0.0.1", 19471, timeout=5)) as connection:
        connection.request("GET", target)
        with connection.getresponse() as response:
            if response.status != 200:
                raise ValueError(f"Jaeger query returned HTTP {response.status}")
            return json.load(response)


def reject_nonfinite(token):
    """Reject numeric constants outside the JSON grammar."""
    raise ValueError(f"Invalid JSON number in OTLP acknowledgement: {token}")


def check_acknowledgement(response):
    """Require a bounded OTLP/JSON full-success reply for the replayed batch."""
    if response.status != 200:
        raise ValueError("OTLP receiver didn't return HTTP 200")
    limit = 8192
    body = response.read(limit + 1)
    if len(body) > limit:
        raise ValueError("OTLP acknowledgement exceeded 8 KiB")
    ack = json.loads(body.decode("utf-8"), parse_constant=reject_nonfinite)
    if not isinstance(ack, dict):
        raise ValueError("Expected an OTLP acknowledgement object")
    partial = ack.get("partialSuccess")
    if partial is None:
        return
    if not isinstance(partial, dict):
        raise ValueError("Malformed OTLP partialSuccess")
    count = partial.get("rejectedSpans")
    zero = count is None or (type(count) is int and count == 0) or count == "0"
    if not zero or partial.get("errorMessage") not in (None, ""):
        raise ValueError("OTLP receiver didn't confirm full acceptance")


def post_fixture(payload, port):
    """Post one projection directly to a loopback viewer and verify its reply."""
    with closing(HTTPConnection("127.0.0.1", port, timeout=5)) as connection:
        body = (json.dumps(payload, separators=(",", ":")) + "\n").encode()
        connection.request("POST", "/v1/traces", body=body, headers={"Content-Type": "application/json"})
        with connection.getresponse() as response:
            check_acknowledgement(response)


def inspect_case(scenario, payload):
    """Compare a recorded trace with its stored spans, events and resource fields."""
    source = payload["resourceSpans"][0]
    trace_id = source["scopeSpans"][0]["spans"][0]["traceId"]
    stored = query("traces/" + trace_id)["result"]["resourceSpans"][0]
    check_fields(source["resource"], stored["resource"])
    if stored["scopeSpans"][0]["scope"] != source["scopeSpans"][0]["scope"]:
        raise ValueError("Instrumentation scope changed")
    before = source["scopeSpans"][0]["spans"]
    after = {span["spanId"]: span for span in stored["scopeSpans"][0]["spans"]}
    if len(before) != len(after):
        raise ValueError(f"Span count changed for {scenario}")
    by_name = {span["name"]: attributes(span) for span in after.values()}
    run = by_name["gaap.agent_run"]
    effect = by_name["gaap.protected_effect"]
    durations = []
    event_count = 0
    verdicts = []
    query_event_order = []
    for span in before:
        actual = after[span["spanId"]]
        for key in ("name", "traceId", "parentSpanId"):
            if actual.get(key) != span.get(key):
                raise ValueError(f"Unexpected {key} in {scenario}")
        start_ns = int(span["startTimeUnixNano"])
        end_ns = int(span["endTimeUnixNano"])
        # Jaeger 2.23.0 normalizes these zero-duration fixtures to one nanosecond.
        expected_end_ns = start_ns + 1 if end_ns == start_ns else end_ns
        if actual["startTimeUnixNano"] != span["startTimeUnixNano"] or actual["endTimeUnixNano"] != str(expected_end_ns):
            raise ValueError(f"Span timestamps changed in {scenario}")
        check_fields(span, actual)
        expected_events = span.get("events", [])
        stored_events = actual.get("events", [])
        if len(expected_events) != len(stored_events):
            raise ValueError("Event count changed")

        def sequence(event):
            """Read the GAAP ledger sequence number for an OTLP event."""
            return int(attributes(event)["gaap.event.sequence"]["intValue"])

        query_event_order.extend(sequence(event) for event in stored_events)
        for expected, event in zip(expected_events, sorted(stored_events, key=sequence)):
            if expected["name"] != event["name"]:
                raise ValueError("Event identity changed")
            if expected["timeUnixNano"] != event["timeUnixNano"]:
                raise ValueError(f"Event timestamp changed in {scenario}")
            check_fields(expected, event)
            if event["name"] == "gaap.verification":
                verdicts.append(attributes(event)["gaap.verification.verdict"]["stringValue"])
        event_count += len(stored_events)
        durations.append(int(actual["endTimeUnixNano"]) - int(actual["startTimeUnixNano"]))
    return {
        "scenario": scenario, "trace_id": trace_id,
        "run": run["gaap.run.terminal_status"]["stringValue"],
        "reason": run["gaap.run.reason"]["stringValue"],
        "decision": effect["gaap.effect.decision"]["stringValue"],
        "effect": effect["gaap.effect.execution_status"]["stringValue"],
        "usage_scope": run["gaap.usage.model_scope"]["stringValue"], "verification_events": verdicts, "span_count": len(after),
        "event_count": event_count, "query_event_order": query_event_order, "returned_durations_ns": durations
    }


def main():
    """Optionally replay the fixtures, verify their traces and check attribute searches."""
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--replay", action="store_true", help="Post the nine fixtures before checking Jaeger")
    modes.add_argument("--post-eight", type=int, choices=(19473, 19572),
                       help="Post only the eight archived runs to this loopback OTLP/HTTP port")
    args = parser.parse_args()
    directory = Path(__file__).resolve().parents[1]
    fixtures = [json.loads(line) for line in (directory / "recorded-runs.ndjson").read_text().splitlines()]
    if len(fixtures) != 8:
        raise ValueError("Expected eight archived runs")
    if args.post_eight:
        for fixture in fixtures:
            post_fixture(fixture["request"], args.post_eight)
        print(f"Received eight full-success acknowledgements from 127.0.0.1:{args.post_eight}")
        return
    pair = json.loads((directory / "paired.json").read_text())
    fixtures.append({"scenario": "Paired", "request": pair})
    checked = []
    times = []
    for fixture in fixtures:
        payload = fixture["request"]
        if args.replay:
            post_fixture(payload, 19473)
        checked.append(inspect_case(fixture["scenario"], payload))
        start = payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["startTimeUnixNano"]
        times.append(datetime.fromtimestamp(int(start) / 1e9, timezone.utc))
    bounds = {
        "query.serviceName": "gaap",
        "query.startTimeMin": (min(times) - timedelta(minutes=1)).isoformat(),
        "query.startTimeMax": (max(times) + timedelta(minutes=1)).isoformat(),
    }
    searches = []
    paired_root = pair["resourceSpans"][0]["scopeSpans"][0]["spans"][0]
    attempt_key = "threadloop.attempt.id_sha256"
    attempt = attributes(paired_root)[attempt_key]["stringValue"]
    for key, value, expected in [
        ("gaap.run.terminal_status", "blocked", [case["trace_id"] for case in checked if case["run"] == "blocked"]),
        ("gaap.run.reason", "fixture.absent", []),
        (attempt_key, attempt, [paired_root["traceId"]]),
    ]:
        result = query("trace-summaries", {**bounds, "query.attributes": json.dumps({key: value})})
        if not isinstance(result, dict) or "error" in result or not isinstance(result.get("summaries"), list):
            raise ValueError("Jaeger search didn't return a summaries array without an error")
        found = sorted(item["traceId"] for item in result["summaries"])
        if found != sorted(expected):
            raise ValueError(f"Search mismatch for {key}")
        searches.append({"attribute": key, "value": value, "matches": len(found)})
    print(json.dumps({"cases": checked, "searches": searches}, indent=2))


if __name__ == "__main__":
    main()
