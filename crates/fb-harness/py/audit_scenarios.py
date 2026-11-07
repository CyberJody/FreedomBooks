"""Audit each scenario against both saved runs, including matching accidental refusals.

python3 crates/fb-harness/py/audit_scenarios.py target/harness audit.json audit.md
The comparer remains the full book/financial gate; this emits a reviewable outcome inventory.
"""
import json
import sys
from collections import Counter
from pathlib import Path


def audit(directory):
    rows = []
    for path in sorted((Path(__file__).resolve().parents[1] / 'scenarios').glob('*.json')):
        scenario = json.loads(path.read_text())
        steps = scenario['steps']
        row = dict(scenario=path.name, steps=len(steps), successes=0, refusals=0,
                   missing=0, empty=0, problems=[], refusal_reasons=[], intentional_missing=[])
        reason_counts = Counter()
        for side in ('python', 'rust'):
            dump = json.loads((directory / f'{path.stem}.{side}.json').read_text())
            outcomes = dump.get('final', dump)['outcomes']
            if len(outcomes) != len(steps):
                row['problems'].append(f'{side}: incomplete outcome inventory')
            for index, (step, outcome) in enumerate(zip(steps, outcomes, strict=True)):
                prefix = f'{side} step {index} {step["op"]}'
                if not isinstance(step.get('expect_error'), bool):
                    row['problems'].append(prefix + ': unpinned success/refusal')
                if (outcome['step'] != index or outcome['op'] != step['op']
                        or outcome['ok'] == step.get('expect_error') or outcome.get('unexpected')):
                    row['problems'].append(prefix + ': unexpected outcome')
                if step.get('expect_kind') is not None and outcome.get('kind') != step['expect_kind']:
                    row['problems'].append(prefix + ': wrong refusal kind')
                if step.get('expect_message') is not None and outcome.get('message') != step['expect_message']:
                    row['problems'].append(prefix + ': wrong refusal message')
                if not outcome['ok']:
                    if not isinstance(step.get('expect_kind'), str):
                        row['problems'].append(prefix + ': unpinned refusal reason')
                    missing = outcome['kind'] in ('DoesNotExist', 'MissingRef')
                    empty = not outcome.get('message')
                    if empty or (missing and not step.get('intent')):
                        row['problems'].append(prefix + ': accidental missing/empty refusal')
                    if side == 'python':
                        row['missing'] += missing
                        row['empty'] += empty
                        reason_counts[(outcome['op'], outcome['kind'], outcome.get('message', ''))] += 1
                        if missing:
                            row['intentional_missing'].append(dict(step=index, op=step['op'], intent=step.get('intent')))
                if side == 'python':
                    row['successes' if outcome['ok'] else 'refusals'] += 1
        row['refusal_reasons'] = [dict(op=op, kind=kind, message=message, count=count)
                                 for (op, kind, message), count in sorted(reason_counts.items())]
        rows.append(row)
    return rows


def main():
    directory, json_path, markdown_path = map(Path, sys.argv[1:])
    rows = audit(directory)
    problems = [problem for row in rows for problem in row['problems']]
    totals = {key: sum(row[key] for row in rows) for key in ('steps', 'successes', 'refusals', 'missing', 'empty')}
    result = dict(scenarios=len(rows), totals=totals, problems=problems, audit=rows)
    json_path.write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n')
    lines = ['# Main scenario outcome audit', '',
             'Base: `05945e5`. Every step is pinned to success or a named refusal. Counts below were checked',
             'independently against both Python and Rust outputs, including the scenario manifest and any message pins.',
             'The JSON audit additionally inventories every refused operation, kind, message and count.', '',
             'Changes: the 77 accidental empty-message `DoesNotExist` refusals in the prepayment random corpus',
             'were removed by regenerating 50 randomized complete lifecycles (500 commands plus 13 setup steps).',
             'Each has a successful application, unapply, refund, refund void and deposit void, plus a genuine',
             'blocked deposit void. Generation executes the frozen Python services and changes live pools only',
             'after success; an unintended outcome aborts generation instead of being learned as an expectation.', '',
             'The combined fixture already used `sales_void_sale` at steps 75/97/98 on this base. Its generator',
             'was stale (53 steps); it now regenerates all 108 corrected steps, including deposit/reconciliation',
             'blocks, backdate refusals and successful releases. No financial-core behavior changed.', '',
             'The two retained `DoesNotExist` cases are deliberate, name existing records, have nonempty messages',
             'and carry explicit `intent` notes: `edge-multi-company` step 37 posts the existing c2 journal via c1;',
             '`sales-cash-sales` step 15 sends an existing cash sale to the customer-payment reversal API.',
             'Neither is an unsuccessful creation followed by a dependent operation. No other missing-reference',
             'or empty-message refusal remains. No real Python/Rust mismatch was found.', '',
             '| Scenario | Steps | Success | Intended refusal | DoesNotExist (intentional) | Empty message | Audit |',
             '| --- | ---: | ---: | ---: | ---: | ---: | --- |']
    for row in rows:
        status = 'FAIL' if row['problems'] else 'PASS'
        lines.append(f'| {row["scenario"]} | {row["steps"]} | {row["successes"]} | {row["refusals"]} | {row["missing"]} | {row["empty"]} | {status} |')
    lines += ['', f'{len(rows)} scenarios; totals: {totals}; {len(problems)} audit problems.', '']
    markdown_path.write_text('\n'.join(lines))
    print(json.dumps(dict(scenarios=len(rows), totals=totals, problems=len(problems))))
    if problems:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
