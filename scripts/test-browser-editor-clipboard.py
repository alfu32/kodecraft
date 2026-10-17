#!/usr/bin/env python3
"""Qualify editor clipboard fallback through real Chrome, xterm and ttyd."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('browser_terminal', ROOT / 'scripts/test-serve-browser.py')
browser_terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(browser_terminal)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ttyd', type=Path, required=True)
    parser.add_argument('--chrome', default=shutil.which('google-chrome') or shutil.which('chromium'))
    args = parser.parse_args()
    assert args.chrome, 'Chrome or Chromium is required'
    ttyd = args.ttyd.resolve(strict=True)
    binary = (ROOT / 'target/debug/kode').resolve(strict=True)
    cases = []
    with tempfile.TemporaryDirectory(prefix='kode-browser-clipboard-') as directory, sync_playwright() as pw:
        base = Path(directory)
        browser = pw.chromium.launch(executable_path=args.chrome, headless=True, args=['--no-sandbox'])
        try:
            for use_ssh in [False, True]:
                project = base / ('ssh-workspace' if use_ssh else 'direct-workspace')
                project.mkdir()
                source = project / 'code.rs'
                source.write_bytes(b'original\n')
                catalog = project / 'lsp.json'
                catalog.write_text('{"servers":[]}')
                env = dict(os.environ, XDG_CONFIG_HOME=str(project / 'config'),
                           KODE_RS_HOME=str(project / 'resources'), KODE_HOME=str(project / 'legacy-resources'),
                           KODE_LSP_CATALOG=str(catalog), KODE_LSP_HOME=str(project / 'lsp'),
                           KODE_RS_DATABASE_HOME=str(project / 'database'),
                           KODE_RS_JDBC_HOME=str(project / 'plugins'),
                           KODE_JAVA=str(project / 'absent-java'), KODE_TTYD=str(ttyd))
                for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS']:
                    env.pop(key, None)
                with browser_terminal.gateway_fixture(binary, project, env, project, use_ssh) as (gateway, url):
                    context = browser.new_context(viewport={'width': 1000, 'height': 650},
                                                  permissions=['clipboard-read', 'clipboard-write'])
                    page = context.new_page()
                    try:
                        page.goto(url)
                        page.wait_for_function('window.term && term.rows > 0')
                        browser_terminal.visible(page, 'Select a file to begin editing.')
                        page.keyboard.press('Enter')
                        browser_terminal.click_text(page, 'code.rs')
                        browser_terminal.visible(page, 'original')
                        page.evaluate("navigator.clipboard.writeText('external sentinel')")
                        page.keyboard.press('Control+a')
                        page.keyboard.press('Control+c')
                        assert page.evaluate('navigator.clipboard.readText()') == 'external sentinel'
                        page.keyboard.press('Control+x')
                        page.keyboard.press('Control+s')
                        browser_terminal.until(lambda: source.read_bytes() == b'', 'cut saved empty file')
                        page.keyboard.press('Control+v')
                        browser_terminal.visible(page, 'original')
                        page.keyboard.press('Control+s')
                        browser_terminal.until(lambda: source.read_bytes() == b'original\n', 'paste restored exact bytes')
                        assert page.evaluate('navigator.clipboard.readText()') == 'external sentinel'
                        children = browser_terminal.child_pids(gateway.pid)
                        assert len(children) == 1, children
                    finally:
                        context.close()
                    browser_terminal.until(lambda: all(not Path(f'/proc/{pid}').exists() for pid in children),
                                           'browser child reaped')
                    cases.append({'transport': 'ssh-forwarded-loopback' if use_ssh else 'loopback',
                                  'status': 'passed', 'cut_saved_empty': True,
                                  'restored_exact_bytes': True, 'browser_clipboard_unchanged': True,
                                  'child_reaped': True})
        finally:
            browser.close()
    report = {'status': 'passed', 'ttyd_version': subprocess.check_output([str(ttyd), '--version'],
               text=True).strip(), 'desktop_clipboard_unavailable': True,
              'chrome': args.chrome, 'cases': cases, 'passed': len(cases)}
    (ROOT / 'docs/parity/E13-browser.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f'Passed {len(cases)} real browser editor clipboard checks')


if __name__ == '__main__':
    main()
