"""Serve one synthetic evidence card from validated coding-inspect output on stdin."""

import argparse
import difflib
import html
import json
import re
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


def escape(value):
    """Escape a fixture value before placing it in HTML."""
    return html.escape(str(value))


def render_card(run):
    """Render the applied shipping diff and verifier observations for one native run."""
    effect, = run["protected_effect_results"]
    report = run["verification"]
    cases = report["cases"]
    if run["path"] != "shipping.py" or not run["terminal_receipt_verified"]:
        raise ValueError("Expected a validated shipping fixture")
    if run["model_usage"] is not None or not report["fresh"]:
        raise ValueError("Unexpected model usage or verifier freshness")
    diff = difflib.unified_diff(
        run["before"].splitlines(), run["after"].splitlines(),
        fromfile="shipping.py (before)", tofile="shipping.py (after)", lineterm="",
    )
    passed = sum(case["passed"] is True for case in cases)
    rows = []
    for case in cases:
        observation = case["observed"]
        actual = observation.get("value", observation.get("error"))
        values = (case["name"], case["quantity"], case["unit_price"], case["expected"],
                  actual, "Pass" if case["passed"] else "Fail")
        rows.append("<tr>" + "".join(f"<td>{escape(v)}</td>" for v in values) + "</tr>")
    return f"""<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Shipping quote verification</title>
<style>
body{{font:16px/1.45 system-ui,sans-serif;max-width:62rem;margin:2rem auto;padding:0 1.5rem;color:#162530}}
h1,h2{{line-height:1.15}} h2{{font-size:1.25rem;margin:1.4rem 0 .5rem}}
p{{margin:.3rem 0 .8rem}} code,pre{{font-family:ui-monospace,monospace}}
code{{overflow-wrap:anywhere}} pre{{padding:.9rem 1rem;background:#f2f5f7;overflow:auto;font-size:.85rem}}
dl{{display:grid;grid-template-columns:max-content auto;gap:.2rem 1.2rem}} dd{{margin:0}} dt{{font-weight:600}}
table{{border-collapse:collapse;width:100%}} th,td{{padding:.35rem .5rem;text-align:left;border-bottom:1px solid #d8dfe3}}
caption{{text-align:left}} th{{background:#edf2f5}} body>p:first-child{{color:#52616c}}
</style><body><p>GAAP &middot; Synthetic coding session</p><h1>Shipping quote verification</h1>
<p>Receipt <code>{escape(run["receipt"]["receipt_digest"])}</code></p>
<dl><dt>Agent Run</dt><dd>{escape(run["receipt"]["body"]["terminal_status"])}</dd>
<dt>Protected effect</dt><dd>{escape(effect["body"]["decision"]["decision"]["outcome"])} decision &middot; {escape(effect["body"]["execution_status"])}</dd>
<dt>Integrity</dt><dd>Native receipt and effect digests validated by coding-inspect</dd>
<dt>Signer authentication</dt><dd>No signature verification in this fixture</dd>
<dt>Model tokens and cost</dt><dd>Unavailable</dd></dl>
<h2>Applied change</h2><pre>{escape(chr(10).join(diff))}</pre>
<h2>Verification cases</h2><p>{passed} of {len(cases)} passed &middot; {report["fixed_case_count"]} fixed cases and {report["sample_count"]} seeded cases</p>
<table><thead><tr><th>Case</th><th>Quantity</th><th>Unit price</th><th>Expected</th><th>Observed</th><th>Result</th></tr></thead>
<tbody>{''.join(rows)}</tbody></table></body></html>""".encode()


def main():
    """Bind the fixture card to loopback at the supplied receipt digest."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("digest", help="Exact sha256 receipt digest for this fixture")
    args = parser.parse_args()
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", args.digest):
        parser.error("Invalid receipt digest")
    run, = json.load(sys.stdin)["runs"]
    if run["receipt"]["receipt_digest"] != args.digest:
        parser.error("Receipt digest mismatch")
    route = "/receipts/" + args.digest.removeprefix("sha256:")
    card = render_card(run)

    class CardHandler(BaseHTTPRequestHandler):
        def do_GET(self):
            """Return the card for its exact host and receipt path, or respond with 404."""
            if self.headers.get("Host") != "127.0.0.1:19490" or self.path != route:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(card)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'")
            self.end_headers()
            self.wfile.write(card)

    with HTTPServer(("127.0.0.1", 19490), CardHandler) as server:
        server.serve_forever()


if __name__ == "__main__":
    main()
