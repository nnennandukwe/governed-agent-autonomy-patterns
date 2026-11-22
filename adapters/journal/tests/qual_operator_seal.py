"""Admit a host observation after the live shipping engine sealed a failed run."""

import contextlib
import json
import os

from qual_history import number_canonical
from qual_host import digest, snapshot
from qual_operator_observer import DirectoryWatch, observe


def check_sealed_failure(case):
    """Admit the observed stage abort and verify both original seals and zero workspace writes on
    lookup and retry.
    """
    f, op = case.fixture("sealed-failure")
    case.converted(f, op)
    target = snapshot(f.target)
    record = observe(f, op, "stopped_before_rename", seal_before_stop=True)
    attempt_id = record["attempt_id"]
    source = op.deposit(record)

    prior_bytes = f.slot.journal.read_bytes()
    journal = json.loads(prior_bytes)
    [original] = journal["attempts"]
    case.assertEqual(original["attempt_id"], attempt_id)
    case.assertEqual([fact["tag"] for fact in original["facts"]], ["intent", "dispatch", "observed", "published"])
    case.assertEqual(original["facts"][-2]["payload"]["disposition"], "stopped_before_rename")
    payload = original["facts"][-1]["payload"]
    case.assertEqual(payload["receipt"]["body"]["terminal_status"], "failed")
    case.assertEqual(payload["result"]["body"]["execution_status"], "failed")
    sealed = {}
    for family, envelope in payload.items():
        case.assertEqual(digest(number_canonical(envelope["body"])), envelope[family + "_digest"])
        sealed[family] = number_canonical(envelope)
        case.assertIn(sealed[family], prior_bytes)

    case.assertEqual(snapshot(f.target), target)
    with contextlib.ExitStack() as stack:
        watch = DirectoryWatch(f.workspace)
        stack.callback(os.close, watch.fd)
        ruling = op.reconcile(attempt_id, source)
        case.assertEqual(ruling["generation"], journal["generation"] + 1)
        case.assertFalse(ruling["workspace_hold"])
        case.assertEqual(ruling["ruling"]["disposition"], "stopped_before_rename")
        case.assertEqual(ruling["ruling"]["attempt_id"], attempt_id)
        case.assertEqual(ruling["ruling"]["evidence_digest"], source)

        confirmed_bytes = f.slot.journal.read_bytes()
        for original_envelope in sealed.values():
            case.assertIn(original_envelope, confirmed_bytes)
        confirmed_anchor = f.slot.anchor.read_bytes()
        lookup = f.lookup(f.key, expected="published")
        code, duplicate = f.submit()
        case.assertEqual(code, 0, duplicate)
        for answer in (lookup, duplicate):
            case.assertEqual(answer["status"], "published")
            case.assertEqual(answer["generation"], ruling["generation"])
            case.assertEqual(answer["rulings"], [ruling["ruling"]])
            case.assertEqual(answer["attempt"], original)
            for family, original_envelope in sealed.items():
                case.assertEqual(number_canonical(answer["attempt"]["facts"][-1]["payload"][family]), original_envelope)
        case.assertEqual(f.slot.anchor.read_bytes(), confirmed_anchor)
        case.assertEqual(f.slot.journal.read_bytes(), confirmed_bytes)
        watch.drain()
        case.assertEqual(watch.events, [])
        case.assertEqual(snapshot(f.target), target)

    f.commands.append({"attempt_id": attempt_id, "source": source, "ruling": ruling,
                       "result_digest": payload["result"]["result_digest"],
                       "receipt_digest": payload["receipt"]["receipt_digest"],
                       "workspace_events_after_seal": watch.events})
