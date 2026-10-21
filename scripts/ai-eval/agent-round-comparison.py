#!/usr/bin/env python3
"""Compare Agent rounds without changing their artifacts.

Usage: agent-round-comparison.py ROUND_2 ROUND_3
Print Markdown tables; percentiles use linear interpolation at (n - 1) * p.
Round 2's optional replay audit supplies selected-repair removal counts.
"""
import json
from collections import Counter
from pathlib import Path
import re
import statistics
import sys


def load(directory, arm):
    rows = []
    for path in sorted(directory.glob(f'*.*.{arm}.json')):
        result = json.loads(path.read_text())
        judge = json.loads(path.with_suffix('.judged.json').read_text())
        rows.append((result, judge))
    assert len(rows) == 36, (directory, arm, len(rows))
    assert len({(r['scenario'], r['trial']) for r, _ in rows}) == 36
    return rows


def table(headers, rows):
    print('| ' + ' | '.join(headers) + ' |')
    print('|' + '|'.join(['---'] * len(headers)) + '|')
    for row in rows:
        print('| ' + ' | '.join(map(str, row)) + ' |')
    print()


def verdicts(rows):
    counts = Counter(e['verdict'] for _, j in rows for e in j['essentials'])
    n = sum(counts.values())
    return ' / '.join(str(counts[k]) for k in ('met', 'partly', 'missed')), 100 * (counts['met'] + .5 * counts['partly']) / n


def percentile(values, p):
    values = sorted(values)
    position = (len(values) - 1) * p
    lower = int(position)
    upper = min(lower + 1, len(values) - 1)
    return values[lower] + (values[upper] - values[lower]) * (position - lower)


def kept(report):
    return int(report.get('summary') is not None) + sum(len(report[k]) for k in ('observations', 'hypotheses', 'recommendations')) + sum(x['kind'] == 'stated' for x in report['limitations'])


def size(draft):
    return 1 + sum(len(draft[k]) for k in ('observations', 'hypotheses', 'recommendations', 'limitations'))


round2, round3 = map(Path, sys.argv[1:])
sets = [('Round 2 agent', load(round2, 'agent')), ('Round 3 agent', load(round3, 'agent')), ('Round 3 unchecked', load(round3, 'unchecked'))]
print('Essential credit (met / partly / missed; credit):\n')
partitions = [('A–L', 'A', 'L'), ('M–O', 'M', 'O'), ('P–R', 'P', 'R'), ('All', 'A', 'R')]
table(['Run'] + [p[0] for p in partitions], [[name] + [f'{v[0]}; {v[1]:.1f}%' for _, lo, hi in partitions for v in [verdicts([(r, j) for r, j in rows if lo <= r['scenario'][0] <= hi])]] for name, rows in sets])
keys = ['factual_correctness', 'recommendation_quality', 'evidence_quality', 'uncertainty_handling', 'user_experience']
table(['Run', 'Correctness', 'Recommendations', 'Evidence', 'Uncertainty', 'UX'], [[name] + [f'{statistics.mean(j["scores"][k] for _, j in rows):.2f}' for k in keys] for name, rows in sets])
print('Per investigation, all A–R (36 per run):\n')
performance = []
for name, rows in sets[:2]:
    for key, title in [('model_calls', 'Model calls'), ('input_tokens', 'Input tokens'), ('output_tokens', 'Output tokens'), ('seconds', 'Seconds'), ('operations', 'Operations')]:
        values = [r['seconds'] if key == 'seconds' else len(r['steps']) if key == 'operations' else r['usage'][key] for r, _ in rows]
        performance.append([name, title] + [f'{v:,.2f}' for v in (statistics.mean(values), percentile(values, .5), percentile(values, .9), max(values))])
table(['Run', 'Metric', 'Mean', 'p50', 'p90', 'Max'], performance)
audit = {a['run']: a for a in json.loads((round2 / '_check-audit.json').read_text())}
losses = []
for name, rows in sets[:2]:
    answered = [r for r, _ in rows if r['draft']]
    removed = drafted = 0
    for r in answered:
        draft = r['draft']
        if name == 'Round 2 agent':
            a = audit[f"{r['scenario']}.{r['trial']}.agent"]
            draft = a['repair_draft'] if a['repair_kept'] else draft
        drafted += size(draft)
        removed += size(draft) - kept(r['report'])
    losses.append([name, len(answered), sum(r['report']['summary'] is None for r in answered), f'{removed} / {drafted} ({100 * removed / drafted:.1f}%)', sum(r['failure'] is None and not r['clarification'] and not any((r['report'] or {}).get(k) for k in ('summary', 'observations', 'hypotheses', 'recommendations')) for r, _ in rows), sum(r['failure'] is not None for r, _ in rows)])
table(['Run', 'Answer drafts', 'Summaries lost', 'Texts removed / drafted', 'Empty completed answers', 'Failures'], losses)
reasons = []
for name, rows in sets[:2]:
    counter = Counter(re.search(r'reason="([^"]+)" part="([^"]+)"', line).groups() for r, _ in rows for line in r['check_log'] if 'Investigation text refused' in line)
    for (reason, part), n in sorted(counter.items()):
        reasons.append([name, reason, part, n])
table(['Run', 'Refusal reason (all check passes)', 'Part', 'Count'], reasons)
table(['Run', 'Trial 1 credit', 'Trial 2 credit'], [[name] + [f'{verdicts([(r,j) for r,j in rows if r["trial"] == t])[1]:.1f}%' for t in (1,2)] for name, rows in sets])
table(['Scenario', 'Round 2 agent', 'Round 3 agent', 'Round 3 unchecked'], [[sets[0][1][i][0]['scenario']] + [verdicts([(r,j) for r,j in rows if r['scenario'] == sets[0][1][i][0]['scenario']])[0] for _, rows in sets] for i in range(0,36,2)])
