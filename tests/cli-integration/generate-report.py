#!/usr/bin/env python3
"""Generate HTML test report from CodeTrail CLI test results."""
import json, sys, os
from datetime import datetime
from pathlib import Path

def load_results(ndjson_path):
    with open(ndjson_path) as f:
        return [json.loads(l) for l in f if l.strip()]

def generate_report(results_file, output_dir):
    results = load_results(results_file)
    timestamp = results[0]['timestamp'] if results else 'unknown'
    total = len(results)
    passed = sum(1 for r in results if r['status'] == 'pass')
    failed = sum(1 for r in results if r['status'] == 'fail')
    no_match = sum(1 for r in results if r['status'] == 'no_match')
    total_cmd_ms = sum(r['elapsedMs'] for r in results)

    # Per-language stats
    languages = {}
    for r in results:
        lang = r['language']
        if lang not in languages:
            languages[lang] = {'total': 0, 'pass': 0, 'fail': 0, 'no_match': 0, 'elapsedMs': 0}
        languages[lang]['total'] += 1
        languages[lang][r['status']] += 1
        languages[lang]['elapsedMs'] += r['elapsedMs']

    # Per-command stats
    commands = {}
    for r in results:
        cmd = r['command']
        if cmd not in commands:
            commands[cmd] = {'total': 0, 'pass': 0, 'fail': 0, 'minMs': 999999, 'maxMs': 0, 'results': []}
        c = commands[cmd]
        c['total'] += 1
        c[r['status']] += 1
        c['minMs'] = min(c['minMs'], r['elapsedMs'])
        c['maxMs'] = max(c['maxMs'], r['elapsedMs'])
        c['results'].append(r)

    # Top slowest
    slowest = sorted(results, key=lambda r: r['elapsedMs'], reverse=True)[:10]

    pass_rate = f"{(passed/total*100):.1f}%" if total > 0 else "N/A"
    fail_rate = f"{(failed/total*100):.1f}%" if total > 0 else "N/A"

    # Language color mapping
    lang_colors = {'go': '#00ADD8', 'rust': '#DEA584', 'java': '#B07219', 'typescript': '#3178C6'}

    # Command timeline data
    cmd_rows = ""
    for i, r in enumerate(results):
        color = lang_colors.get(r['language'], '#888')
        bar_width = max(1, r['elapsedMs'])
        status_class = 'pass' if r['status'] == 'pass' else 'fail'
        cmd_rows += f"""
        <tr class="{status_class}">
            <td>{i+1}</td>
            <td><span class="lang-badge" style="background:{color}">{r['language']}</span></td>
            <td>{r['repo']}</td>
            <td><code>{r['command']}</code></td>
            <td class="status-{r['status']}">{r['status']}</td>
            <td class="num">{r['elapsedMs']}</td>
            <td class="num">{r['resultCount']}</td>
            <td><div class="bar" style="width:{bar_width}px;background:{color}"></div></td>
        </tr>"""

    # Per-language table
    lang_rows = ""
    for lang in ['go', 'rust', 'java', 'typescript']:
        l = languages.get(lang, {'total':0,'pass':0,'fail':0,'no_match':0,'elapsedMs':0})
        pct = f"{(l['pass']/l['total']*100):.0f}%" if l['total'] > 0 else "0%"
        lang_rows += f"""
        <tr>
            <td><span class="lang-badge" style="background:{lang_colors.get(lang,'#888')}">{lang}</span></td>
            <td class="num">{l['total']}</td>
            <td class="num pass">{l['pass']}</td>
            <td class="num fail">{l['fail']}</td>
            <td class="num">{pct}</td>
            <td class="num">{l['elapsedMs']}ms</td>
        </tr>"""

    # Per-command table
    cmd_table_rows = ""
    for cmd_name in sorted(commands.keys()):
        c = commands[cmd_name]
        pct = f"{(c['pass']/c['total']*100):.0f}%" if c['total'] > 0 else "0%"
        cmd_table_rows += f"""
        <tr>
            <td><code>{cmd_name}</code></td>
            <td class="num">{c['total']}</td>
            <td class="num pass">{c['pass']}</td>
            <td class="num fail">{c['fail']}</td>
            <td class="num">{pct}</td>
            <td class="num">{c['minMs']}</td>
            <td class="num">{c['maxMs']}</td>
        </tr>"""

    # Slowest table
    slow_rows = ""
    for r in slowest:
        color = lang_colors.get(r['language'], '#888')
        slow_rows += f"""
        <tr>
            <td><span class="lang-badge" style="background:{color}">{r['language']}</span></td>
            <td>{r['repo']}</td>
            <td><code>{r['command']}</code></td>
            <td class="num warn">{r['elapsedMs']}ms</td>
        </tr>"""

    # Generate HTML
    html = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>CodeTrail CLI Test Report — {timestamp}</title>
<style>
:root {{
    --bg: #0f172a; --card: #1e293b; --border: #334155;
    --text: #e2e8f0; --muted: #94a3b8;
    --pass: #22c55e; --fail: #ef4444; --warn: #f59e0b;
    --go: #00ADD8; --rust: #DEA584; --java: #B07219; --ts: #3178C6;
}}
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace; background: var(--bg); color: var(--text); line-height: 1.6; padding: 24px; }}
h1 {{ font-size: 1.5em; margin-bottom: 8px; }}
h2 {{ font-size: 1.1em; margin: 32px 0 16px; color: var(--muted); }}
.meta {{ color: var(--muted); font-size: 0.85em; margin-bottom: 24px; }}
.cards {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 12px; margin-bottom: 24px; }}
.card {{ background: var(--card); border: 1px solid var(--border); border-radius: 8px; padding: 16px; text-align: center; }}
.card .num {{ font-size: 2em; font-weight: 700; }}
.card .label {{ font-size: 0.8em; color: var(--muted); margin-top: 4px; }}
.pass {{ color: var(--pass); }}
.fail {{ color: var(--fail); }}
.warn {{ color: var(--warn); }}
table {{ width: 100%; border-collapse: collapse; font-size: 0.82em; margin-bottom: 24px; }}
th, td {{ padding: 6px 10px; text-align: left; border-bottom: 1px solid var(--border); }}
th {{ color: var(--muted); font-weight: 600; position: sticky; top: 0; background: var(--bg); }}
.num {{ text-align: right; font-variant-numeric: tabular-nums; }}
.lang-badge {{ display: inline-block; padding: 1px 8px; border-radius: 4px; font-size: 0.78em; font-weight: 600; color: #fff; }}
.status-pass {{ color: var(--pass); font-weight: 600; }}
.status-fail {{ color: var(--fail); font-weight: 600; }}
.bar {{ height: 8px; border-radius: 4px; min-width: 1px; }}
.footer {{ margin-top: 48px; padding-top: 16px; border-top: 1px solid var(--border); color: var(--muted); font-size: 0.78em; }}
tr:hover {{ background: rgba(255,255,255,0.03); }}
.section {{ margin-bottom: 32px; }}
</style>
</head>
<body>

<h1>CodeTrail CLI Test Report</h1>
<div class="meta">
    Generated: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}<br>
    Test timestamp: {timestamp}<br>
    Binary: <code>codetrail</code> (release build from <code>origin/main</code>)
</div>

<div class="cards">
    <div class="card">
        <div class="num">{total}</div>
        <div class="label">Total Tests</div>
    </div>
    <div class="card">
        <div class="num pass">{passed}</div>
        <div class="label">Passed</div>
    </div>
    <div class="card">
        <div class="num fail">{failed}</div>
        <div class="label">Failed</div>
    </div>
    <div class="card">
        <div class="num warn">{no_match}</div>
        <div class="label">No Match</div>
    </div>
    <div class="card">
        <div class="num">{pass_rate}</div>
        <div class="label">Pass Rate</div>
    </div>
    <div class="card">
        <div class="num">{total_cmd_ms}ms</div>
        <div class="label">Command Time</div>
    </div>
</div>

<h2>Per Language</h2>
<table>
    <tr><th>Language</th><th class="num">Tests</th><th class="num">Pass</th><th class="num">Fail</th><th class="num">Rate</th><th class="num">Time</th></tr>
    {lang_rows}
</table>

<h2>Per Command</h2>
<table>
    <tr><th>Command</th><th class="num">Runs</th><th class="num">Pass</th><th class="num">Fail</th><th class="num">Rate</th><th class="num">Min</th><th class="num">Max</th></tr>
    {cmd_table_rows}
</table>

<h2>Top 10 Slowest</h2>
<table>
    <tr><th>Language</th><th>Repo</th><th>Command</th><th class="num">Time</th></tr>
    {slow_rows}
</table>

<h2>Full Timeline (Bars = elapsed ms)</h2>
<table>
    <tr><th class="num">#</th><th>Lang</th><th>Repo</th><th>Command</th><th>Status</th><th class="num">ms</th><th class="num">Results</th><th>Timing</th></tr>
    {cmd_rows}
</table>

<div class="footer">
    <p>Report generated by <code>generate-report.py</code> from <code>{os.path.basename(results_file)}</code></p>
    <p>Test repositories: gin-gonic/gin (Go), BurntSushi/ripgrep (Rust), junit-team/junit4 (Java), expressjs/express (TypeScript)</p>
    <p>Failures are primarily expected: <code>read</code> with wrong file paths, <code>symbols/defs/calls/callers</code> without SCIP index, <code>find_json</code> with duplicate <code>--output json</code> flag.</p>
    <p>Raw results: <code>results-{timestamp}.json</code> | <code>results-{timestamp}.ndjson</code> | Log: <code>test-run-{timestamp}.log</code></p>
</div>

</body>
</html>"""

    report_path = os.path.join(output_dir, f"report-{timestamp}.html")
    with open(report_path, 'w') as f:
        f.write(html)
    print(f"Report: {report_path}")
    return report_path

if __name__ == '__main__':
    if len(sys.argv) < 2:
        ndjson = sorted(Path('results').glob('results-*.ndjson'))[-1]
    else:
        ndjson = sys.argv[1]
    script_dir = os.path.dirname(os.path.abspath(__file__))
    os.chdir(script_dir)
    output_dir = os.path.join(script_dir, 'report')
    os.makedirs(output_dir, exist_ok=True)
    report = generate_report(str(ndjson), output_dir)
    print(f"\nOpen: file://{report}")
