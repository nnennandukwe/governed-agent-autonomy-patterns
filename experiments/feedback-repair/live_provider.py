"""Send frozen Messages requests and reconcile receipts with Anthropic's native reports."""

from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from decimal import Decimal
import hashlib
import hmac
import http.client
import math
import os
from pathlib import Path
import re
import signal
import ssl
import time
from urllib.parse import urlencode

import corpus


HOST = "api.anthropic.com"
API_VERSION = "2026-10-21"
ANALYTICS_BETA = "analytics-usage-cost-2026-08-24"
BUDGET = corpus.decode((Path(__file__).with_name("budget.json")).read_bytes())
PROFILE = BUDGET["price_snapshot"]
MAX_OUTPUT = 2048
INPUT_MARGIN = 256
MAX_BODY = 512 * 1024
MAX_CAPTURE = 8 * 1024 * 1024
MAX_PAGES = 16
MINUTE = timedelta(minutes=1)


class ProviderError(ValueError):
    """Hold collection when native transport, attribution or evidence can't be verified."""


class ReservationError(ProviderError):
    """The remaining pre-dispatch capacity can't cover this request."""


def _require(condition, message):
    """Raise a safe diagnostic without echoing native bodies or credential values."""
    if not condition:
        raise ProviderError(message)


def _object(raw, limit=MAX_BODY):
    """Read one bounded JSON object, rejecting duplicate names and malformed values."""
    try:
        value = corpus.decode(raw, limit)
    except (ValueError, TypeError, UnicodeError):
        raise ProviderError("Invalid native JSON.") from None
    _require(type(value) is dict, "Expected a native JSON object.")
    return value


def _integer(value):
    """Accept only native nonnegative integer counters."""
    _require(type(value) is int and value >= 0, "Invalid native counter.")
    return value


def _tag(value, prefix):
    """Require a bounded provider ID with its expected type prefix."""
    _require(isinstance(value, str) and re.fullmatch(prefix + r"[A-Za-z0-9_-]{1,128}", value),
             "Missing or invalid provider identifier.")
    return value


def read_account_scope():
    """Read the designated billing dimensions from nonsecret environment bindings."""
    return (_tag(os.getenv("ANTHROPIC_WORKSPACE_ID"), "wrkspc_"),
            _tag(os.getenv("ANTHROPIC_API_KEY_ID"), "apikey_"))


def _credential(admin):
    """Read only the credential for the requested provider role."""
    name = "ANTHROPIC_ADMIN_API_KEY" if admin else "ANTHROPIC_API_KEY"
    value = os.getenv(name)
    _require(value and len(value) <= 2048 and value.isascii() and
             all(33 <= ord(char) <= 126 for char in value), "Missing or invalid " + name + ".")
    return value


def _fingerprint(credential):
    """Derive a study-specific binding that can't authenticate a provider request."""
    return hmac.digest(credential.encode("ascii"), b"GAAP feedback-repair serving key v1", "sha256").hex()


def serving_credential_fingerprint():
    """Bind the serving key at activation without retaining its secret bytes."""
    return _fingerprint(_credential(False))


def _bound_serving_key(expected):
    """Read once and pass that exact verified key through to the HTTP header."""
    key = _credential(False)
    _require(isinstance(expected, str) and re.fullmatch(r"[0-9a-f]{64}", expected) and
             hmac.compare_digest(expected, _fingerprint(key)), "Serving credential changed.")
    return key


def _timeout(_signum, _frame):
    """Interrupt DNS, TLS and slow header or body reads at the absolute deadline."""
    raise ProviderError("Native request deadline expired.")


@contextmanager
def _deadline_guard(deadline):
    """Install a bounded wall timer for this serial POSIX controller request."""
    left = deadline - time.monotonic()
    _require(math.isfinite(left) and left > 0, "Native request deadline expired.")
    _require(signal.getitimer(signal.ITIMER_REAL) == (0, 0), "Native request timer unavailable.")
    previous = signal.signal(signal.SIGALRM, _timeout)
    try:
        signal.setitimer(signal.ITIMER_REAL, left)
        yield left
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def _request(method, path, wire, deadline, *, admin=False, workspace_id=None, serving_key=None):
    """Use one direct, certificate-verified HTTPS connection with no redirect or retry."""
    headers = {"anthropic-version": API_VERSION, "accept": "application/json",
               "accept-encoding": "identity"}
    if admin:
        _require(serving_key is None, "Admin transport requires separate authorization.")
        headers["x-api-key"] = _credential(True)
        headers["anthropic-beta"] = ANALYTICS_BETA
    else:
        _require(isinstance(serving_key, str) and serving_key, "Missing verified serving credential.")
        headers["x-api-key"] = serving_key
        headers["anthropic-workspace-id"] = _tag(workspace_id, "wrkspc_")
        headers["content-type"] = "application/json"
    # PROTOCOL_TLS_CLIENT verifies hostnames. Use system roots without SSLKEYLOGFILE,
    # proxy variables or caller-supplied CA bundles.
    with _deadline_guard(deadline) as left:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        paths = ssl.get_default_verify_paths()
        context.load_verify_locations(cafile=paths.openssl_cafile, capath=paths.openssl_capath)
        connection = http.client.HTTPSConnection(HOST, context=context, timeout=left)
        try:
            connection.request(method, path, body=wire, headers=headers)
            response = connection.getresponse()
            identity = {name: response.getheader(name) for name in
                        ("request-id", "anthropic-workspace-id")}
            body = bytearray()
            try:
                while len(body) <= MAX_BODY:
                    chunk = response.read1(min(65536, MAX_BODY + 1 - len(body)))
                    if not chunk:
                        break
                    body.extend(chunk)
            except http.client.IncompleteRead as error:
                body.extend(error.partial[:MAX_BODY + 1 - len(body)])
                identity["body_status"] = "incomplete"
            except (OSError, http.client.HTTPException, ProviderError):
                identity["body_status"] = "interrupted"
            if len(body) > MAX_BODY:
                identity["body_status"] = "oversized"
            elif response.length:
                identity["body_status"] = "incomplete"
            return response.status, identity, bytes(body[:MAX_BODY])
        except (OSError, http.client.HTTPException):
            raise ProviderError("Native transport failed. Delivery is unknown.") from None
        finally:
            connection.close()


def _list_price(input_tokens, output_tokens):
    """Compute list-price microdollars from the frozen uncached token rates."""
    return (input_tokens * PROFILE["input_microusd_per_token"] +
            output_tokens * PROFILE["output_microusd_per_token"])


def _envelope(user_text):
    """Wrap a single public input in the same tool-free native request for every arm."""
    return {"model": PROFILE["model"], "messages": [{"role": "user", "content": user_text}],
            "thinking": {"type": PROFILE["thinking"]}, "output_config": {"effort": PROFILE["effort"]}}


def _paid_fields():
    """Pin every generation and billing setting excluded from the free count."""
    return {"service_tier": PROFILE["service_tier"], "inference_geo": PROFILE["inference_geo"],
            "max_tokens": MAX_OUTPUT}


def _response_id(status, headers, workspace):
    """Check the serving workspace and native request identity before using the result."""
    _require(status == 200, "Native request didn't return HTTP 200.")
    _require("body_status" not in headers, "Native response body was incomplete or oversized.")
    _require(headers.get("anthropic-workspace-id") == workspace, "Serving workspace changed.")
    return _tag(headers.get("request-id"), "req_")


def prepare_message(public_request, remaining_tokens, deadline, *, expected_fingerprint):
    """Count the identical text for free and return exact bytes and their reservation."""
    workspace, key = read_account_scope()
    serving_key = _bound_serving_key(expected_fingerprint)
    _require(type(public_request) is dict, "Expected one host-projected public request.")
    remaining = min(_integer(remaining_tokens), BUDGET["per_cell_limits"]["total_tokens"])
    envelope = _envelope(corpus.encode(public_request).decode())
    wire = corpus.encode({**envelope, **_paid_fields()})
    # Text-only requests have at most one token per UTF-8 byte before provider framing.
    # Cover the larger of the complete envelope and native estimate, plus framing slack.
    if len(wire) + INPUT_MARGIN + MAX_OUTPUT > remaining:
        raise ReservationError("The request exceeds remaining tokens.")
    status, headers, body = _request("POST", "/v1/messages/count_tokens", corpus.encode(envelope),
                                     deadline, workspace_id=workspace, serving_key=serving_key)
    request_id = _response_id(status, headers, workspace)
    count = _integer(_object(body).get("input_tokens"))
    bound = max(len(wire), count) + INPUT_MARGIN
    if bound + MAX_OUTPUT > remaining:
        raise ReservationError("The native count exceeds remaining tokens.")
    return wire, {"workspace_id": workspace, "key_id": key, "count_request_id": request_id,
                  "credential_fingerprint": expected_fingerprint, "wire_sha256": corpus.digest(wire),
                  "count_input_tokens": count, "input_tokens": bound, "output_tokens": MAX_OUTPUT,
                  "total_tokens": bound + MAX_OUTPUT, "list_microusd": _list_price(bound, MAX_OUTPUT)}


def send_message(wire, deadline, *, reservation):
    """Send the ledger's exact reserved bytes once and return raw evidence before parsing."""
    workspace, key = read_account_scope()
    _require(type(reservation) is dict and reservation.get("workspace_id") == workspace and
             reservation.get("key_id") == key and reservation.get("wire_sha256") == corpus.digest(wire),
             "Request bytes or account scope changed after reservation.")
    serving_key = _bound_serving_key(reservation.get("credential_fingerprint"))
    payload = _object(wire, BUDGET["per_cell_limits"]["total_tokens"])
    messages = payload.get("messages")
    _require(type(messages) is list and len(messages) == 1 and type(messages[0]) is dict,
             "Expected one tool-free message.")
    text = messages[0].get("content")
    _require(type(text) is str and wire == corpus.encode({**_envelope(text), **_paid_fields()}),
             "Native request differs from the frozen profile.")
    return _request("POST", "/v1/messages", wire, deadline, workspace_id=workspace, serving_key=serving_key)


def _zero_usage(usage, *, message):
    """Reject cache or server-tool charges outside the approved service."""
    _require(type(usage) is dict, "Missing native usage.")
    shared = {"cache_creation", "server_tool_use", "cache_read_input_tokens", "output_tokens"}
    metadata = ({"input_tokens", "cache_creation_input_tokens", "inference_geo", "service_tier",
                 "speed", "output_tokens_details"} if message else
                {"uncached_input_tokens", "requests", "total_tokens"})
    _require(usage.keys() <= shared | metadata, "Unrecognized native usage field.")
    for field in ("cache_creation", "server_tool_use"):
        counters = usage.get(field)
        # A Messages reply can explicitly report null for an unused feature.
        if message and field in usage and counters is None:
            continue
        _require(type(counters) is dict and all(_integer(v) == 0 for v in counters.values()),
                 "Unexpected cache or server-tool usage.")
    _require(_integer(usage.get("cache_read_input_tokens")) == 0, "Unexpected cached input.")
    if message:
        _require(_integer(usage.get("cache_creation_input_tokens")) == 0, "Unexpected cache writes.")


def parse_response(status, headers, body, reservation):
    """Settle only authenticated native usage, retaining the model's explicit stop reason."""
    request_id = _response_id(status, headers, reservation["workspace_id"])
    data = _object(body)
    _require(data.get("type") == "message" and data.get("role") == "assistant" and
             data.get("model") == PROFILE["model"], "Unexpected native message identity.")
    message_id = _tag(data.get("id"), "msg_")
    usage = data.get("usage")
    _zero_usage(usage, message=True)
    _require((usage.get("inference_geo"), usage.get("service_tier"), usage.get("speed")) ==
             (PROFILE["inference_geo"], "standard", "standard"), "Native serving profile changed.")
    input_tokens = _integer(usage.get("input_tokens"))
    output_tokens = _integer(usage.get("output_tokens"))
    _require(0 < input_tokens <= reservation["input_tokens"] and
             output_tokens <= reservation["output_tokens"], "Native usage exceeded its reservation.")
    reason = data.get("stop_reason")
    outcomes = {"end_turn": "complete", "max_tokens": "incomplete",
                "model_context_window_exceeded": "incomplete", "refusal": "refused"}
    _require(isinstance(reason, str) and reason in outcomes, "Unknown or incompatible stop reason.")
    content = data.get("content")
    _require(type(content) is list, "Missing native content blocks.")
    texts = []
    for block in content:
        _require(type(block) is dict and block.get("type") in ("text", "thinking", "redacted_thinking"),
                 "Unexpected native content block.")
        if block["type"] == "text":
            _require(type(block.get("text")) is str, "Invalid native text block.")
            texts.append(block["text"])
    return "".join(texts).encode(), {"input_tokens": input_tokens, "output_tokens": output_tokens,
            "list_microusd": _list_price(input_tokens, output_tokens), "message_id": message_id,
            "request_id": request_id, "stop_reason": reason, "outcome": outcomes[reason]}


def _iso(value):
    """Format UTC instants for the native inclusive start and exclusive end."""
    return value.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def _instant(value):
    """Parse a timestamp with an explicit UTC offset."""
    _require(type(value) is str and len(value) <= 40, "Invalid native timestamp.")
    try:
        instant = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        raise ProviderError("Invalid native timestamp.") from None
    _require(instant.utcoffset() == timedelta(0), "Native timestamp must use UTC.")
    return instant


def _scope(workspace_id, key_id, started_at, ended_at):
    """Cover both issued instants with complete UTC minutes on the exclusive account key."""
    for point in (started_at, ended_at):
        _require(isinstance(point, datetime) and point.utcoffset() == timedelta(0),
                 "Expected an aware UTC billing boundary.")
    _require(started_at <= ended_at, "Billing interval runs backwards.")
    floor = started_at.replace(second=0, microsecond=0)
    stop = ended_at.replace(second=0, microsecond=0) + MINUTE
    _require(stop - floor <= timedelta(days=1), "Billing scope exceeds the native 1-minute range.")
    return {"workspace_id": _tag(workspace_id, "wrkspc_"), "api_key_id": _tag(key_id, "apikey_"),
            "starting_at": _iso(floor), "ending_at": _iso(stop)}


def _report_path(kind, scope, cursor):
    """Group by the actual billing dimensions and expose foreign activity on this key."""
    params = {"starting_at": scope["starting_at"], "ending_at": scope["ending_at"],
              "workspace_ids[]": scope["workspace_id"], "api_key_ids[]": scope["api_key_id"],
              "group_by[]": ["workspace_id", "api_key_id", "model", "service_tier", "inference_geo"],
              "time_bucket_width": "1m", "limit": 100, "order_by": "time", "order": "asc"}
    if cursor:
        params["page"] = cursor
    return "/v1/organizations/analytics/" + kind + "?" + urlencode(params, doseq=True)


def _cursor(page, seen):
    """Require an explicit end or a new bounded continuation token."""
    more = page.get("has_more")
    _require(type(more) is bool and "next_page" in page, "Incomplete native pagination.")
    cursor = page["next_page"]
    if not more:
        _require(cursor is None, "Unexpected final pagination cursor.")
        return None
    _require(isinstance(cursor, str) and 0 < len(cursor) <= 512 and
             cursor.isascii() and cursor.isprintable() and cursor not in seen,
             "Repeated or invalid native pagination cursor.")
    seen.add(cursor)
    return cursor


def capture_billing(workspace_id, key_id, started_at, ended_at, deadline):
    """Collect credential-free native evidence on the separately authorized admin host."""
    scope = _scope(workspace_id, key_id, started_at, ended_at)
    records = {}
    size = 0
    for kind in ("usage", "cost"):
        pages = []
        cursor, seen = None, set()
        while len(pages) < MAX_PAGES:
            path = _report_path(kind, scope, cursor)
            status, headers, raw = _request("GET", path, None, deadline, admin=True)
            _require(status == 200 and "body_status" not in headers, "Admin analytics query failed.")
            request_id = _tag(headers.get("request-id"), "req_")
            native = _object(raw)
            size += len(raw)
            _require(size <= MAX_CAPTURE, "Native report exceeds the capture limit.")
            pages.append({"path": path, "request_id": request_id, "body": raw.decode()})
            cursor = _cursor(native, seen)
            if cursor is None:
                break
        _require(cursor is None, "Native report exceeds the pagination limit.")
        records[kind] = pages
    encoded = corpus.encode({"format": "anthropic-usage-cost/1", "host": HOST,
                             "version": API_VERSION, "beta": ANALYTICS_BETA,
                             "scope": scope, "pages": records})
    _require(len(encoded) <= MAX_CAPTURE, "Native capture exceeds the byte limit.")
    return encoded


def _cents(value):
    """Read nonnegative USD cents at the provider's six-decimal precision."""
    _require(isinstance(value, str) and re.fullmatch(r"[0-9]{1,12}\.[0-9]{6}", value),
             "Invalid native USD cents.")
    return Decimal(value)


def _rows(kind, pages, scope):
    """Validate interval coverage, native cursors, row identity and snapshot freshness."""
    start, stop = _instant(scope["starting_at"]), _instant(scope["ending_at"])
    _require(type(pages) is list and 0 < len(pages) <= MAX_PAGES, "Missing or unbounded report.")
    cursor, refreshed, seen, rows = None, None, set(), {}
    expected_group = {"workspace_id": scope["workspace_id"], "api_key_id": scope["api_key_id"],
                      "model": PROFILE["model"], "service_tier": "standard",
                      "inference_geo": PROFILE["inference_geo"]}
    for index, page in enumerate(pages):
        _require(type(page) is dict and page.get("path") == _report_path(kind, scope, cursor),
                 "Native report page scope changed.")
        _tag(page.get("request_id"), "req_")
        _require(type(page.get("body")) is str, "Missing raw native page.")
        native = _object(page["body"].encode())
        watermark = _instant(native.get("data_refreshed_at"))
        _require(watermark >= stop and (refreshed is None or refreshed == watermark),
                 "Native report hasn't refreshed through the full interval.")
        refreshed = watermark
        data = native.get("data")
        _require(type(data) is list and len(data) <= 100, "Invalid native page rows.")
        for row in data:
            _require(type(row) is dict and type(row.get("group")) is dict, "Invalid native row.")
            group = row["group"]
            _require(all(group.get(key) == value for key, value in expected_group.items()),
                     "Activity outside the frozen model, key, workspace or tier.")
            bucket = _instant(row.get("starting_at"))
            end = _instant(row.get("ending_at"))
            _require(start <= bucket < stop and not bucket.second and not bucket.microsecond and
                     end == bucket + MINUTE and bucket not in rows, "Invalid or duplicated time bucket.")
            metrics = row.get(kind)
            _require(type(metrics) is dict, "Missing native row metrics.")
            rows[bucket] = metrics
        cursor = _cursor(native, seen)
        _require((index == len(pages) - 1) == (cursor is None), "Missing or trailing native pages.")
    return rows, _iso(refreshed)


def fetch_billing(workspace_id, key_id, started_at, ended_at, deadline, *,
                  capture=None, trusted_sha256=None):
    """Read live admin reports or replay exact bytes pinned by an independent admin handoff."""
    scope = _scope(workspace_id, key_id, started_at, ended_at)
    if capture is None:
        _require(trusted_sha256 is None, "Missing the pinned report capture.")
        capture = capture_billing(workspace_id, key_id, started_at, ended_at, deadline)
    else:
        _require(type(capture) is bytes and len(capture) <= MAX_CAPTURE and
                 isinstance(trusted_sha256, str) and re.fullmatch(r"[0-9a-f]{64}", trusted_sha256)
                 and hmac.compare_digest(hashlib.sha256(capture).hexdigest(), trusted_sha256),
                 "Admin capture doesn't match the independent digest.")
    packet = _object(capture, MAX_CAPTURE)
    _require((packet.get("format"), packet.get("host"), packet.get("version"), packet.get("beta"),
              packet.get("scope")) == ("anthropic-usage-cost/1", HOST, API_VERSION, ANALYTICS_BETA, scope),
             "Native capture identity or interval changed.")
    pages = packet.get("pages")
    _require(isinstance(pages, dict) and pages.keys() == {"usage", "cost"}, "Missing native reports.")
    usage_rows, usage_refreshed = _rows("usage", pages["usage"], scope)
    cost_rows, cost_refreshed = _rows("cost", pages["cost"], scope)
    _require(usage_rows.keys() == cost_rows.keys(), "Native usage and cost cover different minutes.")
    requests = input_tokens = output_tokens = 0
    list_cents = billed_cents = Decimal(0)
    all_rows_at_list = True
    for bucket, row in usage_rows.items():
        _zero_usage(row, message=False)
        uncached = _integer(row.get("uncached_input_tokens"))
        produced = _integer(row.get("output_tokens"))
        _require(_integer(row.get("total_tokens")) == uncached + produced, "Unattributed native tokens.")
        count = _integer(row.get("requests"))
        cost = cost_rows[bucket]
        _require(cost.keys() == {"amount", "list_amount", "requests", "currency"} and
                 cost["currency"] == "USD", "Unsupported native cost fields or currency.")
        listed, billed = _cents(cost["list_amount"]), _cents(cost["amount"])
        _require(count == _integer(cost["requests"]), "Native request counts differ for this minute.")
        _require(listed * 10000 == _list_price(uncached, produced) and billed <= listed,
                 "Native price or billed cost differs for this minute.")
        all_rows_at_list = all_rows_at_list and billed == listed
        requests += count
        input_tokens += uncached
        output_tokens += produced
        list_cents += listed
        billed_cents += billed
    list_micro = list_cents * 10000
    billed_micro = billed_cents * 10000
    _require(billed_micro <= BUDGET["study_limits"]["proposed_cost_stop_microusd"],
             "Native cost exceeds list price or the study ceiling.")
    # The provider can report hundredths of a microdollar after discounts.
    return {"requests": requests, "input_tokens": input_tokens, "output_tokens": output_tokens,
            "list_microusd": int(list_micro), "all_rows_at_list": all_rows_at_list,
            "billed_cents": format(billed_cents, ".6f"),
            "billed_microusd": int(billed_micro) if billed_micro == billed_micro.to_integral() else None,
            "usage_refreshed_at": usage_refreshed, "cost_refreshed_at": cost_refreshed,
            "capture_sha256": hashlib.sha256(capture).hexdigest(), "capture": packet}


def reconcile_billing(report, expected):
    """Match every observed call, token class and list dollar to independent native totals."""
    for field in ("requests", "input_tokens", "output_tokens", "list_microusd"):
        _require(_integer(expected.get(field)) == _integer(report.get(field)),
                 "Native analytics differs from the study ledger.")
    return report
