"""Compare recorded GAAP projections with Jaeger's v3 query response."""

import argparse
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import ProxyHandler, Request, build_opener


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
    url = f"http://127.0.0.1:19471/api/v3/{path}"
    if params:
        url += "?" + urlencode(params)
    client = build_opener(ProxyHandler({}))
    with client.open(url, timeout=5) as response:
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
    parser.add_argument("--replay", action="store_true", help="Post the nine fixtures before checking them")
    replay = parser.parse_args().replay
    directory = Path(__file__).resolve().parents[1]
    fixtures = [json.loads(line) for line in (directory / "recorded-runs.ndjson").read_text().splitlines()]
    pair = json.loads((directory / "paired.json").read_text())
    fixtures.append({"scenario": "Paired", "request": pair})
    checked = []
    times = []
    client = build_opener(ProxyHandler({}))
    for fixture in fixtures:
        payload = fixture["request"]
        if replay:
            request = Request("http://127.0.0.1:19473/v1/traces", data=json.dumps(payload).encode(),
                              headers={"Content-Type": "application/json"})
            with client.open(request, timeout=5) as response:
                check_acknowledgement(response)
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
        found = sorted(item["traceId"] for item in result.get("summaries", []))
        if found != sorted(expected):
            raise ValueError(f"Search mismatch for {key}")
        searches.append({"attribute": key, "value": value, "matches": len(found)})
    print(json.dumps({"cases": checked, "searches": searches}, indent=2))


if __name__ == "__main__":
    main()
