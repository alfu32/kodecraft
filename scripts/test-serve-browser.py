#!/usr/bin/env python3
"""Exercise real ttyd/xterm/Chrome against isolated Rust workspaces (Linux).

Requires Python playwright, Chrome, and KODE_TTYD or --ttyd. No desktop clipboard,
developer configuration, or credentials are used. All authentication is synthetic.
"""
import argparse
import base64
from contextlib import ExitStack, contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import pwd
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
SCREEN = "Array.from({length:term.rows},(_,i)=>term.buffer.active.getLine(i+term.buffer.active.viewportY)?.translateToString(true)||'').join('\\n')"


def until(predicate, label, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.03)
    raise AssertionError(f"Timeout: {label}")


def visible(page, text):
    page.wait_for_function("text => window.term && (" + SCREEN + ").includes(text)", arg=text, timeout=10000)


def click_text(page, text):
    visible(page, text)
    rows = page.evaluate(SCREEN).splitlines()
    y, x = next((y, line.index(text)) for y, line in enumerate(rows) if text in line)
    box = page.locator('.xterm-screen').bounding_box()
    cols, count = page.evaluate('[term.cols,term.rows]')
    page.mouse.click(box['x'] + (x + .5) * box['width'] / cols,
                     box['y'] + (y + .5) * box['height'] / count)


def child_pids(parent):
    """Actual Rust children, including those below an AppImage runtime wrapper."""
    try:
        owner = Path(f'/proc/{parent}/exe').resolve(strict=True)
    except OSError:
        return []
    pending = [parent]
    result = []
    while pending:
        current = pending.pop()
        children = Path(f'/proc/{current}/task/{current}/children')
        try:
            descendants = [int(value) for value in children.read_text().split()]
        except OSError:
            continue
        pending.extend(descendants)
        for pid in descendants:
            try:
                executable = Path(f'/proc/{pid}/exe').resolve(strict=True)
                if executable.name == 'kode' and executable != owner:
                    result.append(pid)
            except OSError:
                pass
    return result


@contextmanager
def ssh_forward(base, destination_port, enabled):
    if not enabled:
        yield destination_port
        return
    directory = base / 'ssh'
    directory.mkdir()
    for name in ['client', 'host']:
        subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(directory / name)], check=True)
    ports = []
    for _ in range(2):
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            ports.append(listener.getsockname()[1])
    ssh_port, forwarded = ports
    user = pwd.getpwuid(os.getuid()).pw_name
    config = directory / 'sshd_config'
    config.write_text(f'Port {ssh_port}\nListenAddress 127.0.0.1\nHostKey {directory}/host\nPidFile {directory}/pid\nAuthorizedKeysFile {directory}/client.pub\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\nAllowUsers {user}\nLogLevel ERROR\n')
    with (directory / 'log').open('wb') as log:
        daemon = subprocess.Popen(['/usr/sbin/sshd', '-D', '-e', '-f', str(config)], stdout=log, stderr=log)
        tunnel = None
        try:
            def listening(port):
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=.1):
                        return True
                except OSError:
                    return False
            until(lambda: listening(ssh_port), 'isolated SSH server')
            tunnel = subprocess.Popen(['ssh', '-F', '/dev/null', '-o', 'StrictHostKeyChecking=no', '-o', f'UserKnownHostsFile={directory}/known', '-o', 'BatchMode=yes', '-o', 'IdentitiesOnly=yes', '-o', 'ExitOnForwardFailure=yes', '-o', 'LogLevel=ERROR', '-i', str(directory / 'client'), '-p', str(ssh_port), '-N', '-L', f'127.0.0.1:{forwarded}:127.0.0.1:{destination_port}', f'{user}@127.0.0.1'], stdout=log, stderr=log)
            until(lambda: listening(forwarded), 'real SSH local forwarding')
            yield forwarded
        finally:
            if tunnel:
                tunnel.terminate()
                tunnel.wait(timeout=10)
            daemon.terminate()
            daemon.wait(timeout=10)


@contextmanager
def gateway_fixture(binary, project, env, base, use_ssh=False):
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    with (base / 'gateway.log').open('wb') as log:
        gateway = subprocess.Popen([str(binary), 'serve', '--port', str(port), str(project)], env=env, stdout=log, stderr=log)
        try:
            url = f'http://127.0.0.1:{port}'

            def ready():
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=.1):
                        return True
                except OSError:
                    return False

            until(ready, 'gateway listening')
            with ssh_forward(base, port, use_ssh) as forwarded:
                yield gateway, f'http://127.0.0.1:{forwarded}'
        finally:
            gateway.terminate()
            gateway.wait(timeout=10)


def qualify_stream(pw, args, binary, base, env, cases):
    case = base / 'stream'
    project = case / 'workspace'
    project.mkdir(parents=True)
    note = project / 'note.txt'
    note.write_text('stream note\n')
    disconnected = threading.Event()
    hits = []

    class Handler(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def do_GET(self):
            hits.append(self.path)
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            try:
                while True:
                    self.wfile.write(b'data: browser-stream-event\n\n')
                    self.wfile.flush()
                    time.sleep(.05)
            except (BrokenPipeError, ConnectionResetError):
                disconnected.set()

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    collection = {'info': {'name': 'Browser fixture'}, 'item': [{'name': 'Browser SSE', 'request': {'method': 'GET', 'url': f'sse://127.0.0.1:{server.server_port}/stream'}}]}
    (project / '.kode-rs.json').write_text(json.dumps({'version': 1, 'restApi': {'collection': collection}, 'activeOrigin': {'Virtual': {'owner': 'rest-node', 'key': '0', 'label': 'REST request'}}}))
    try:
        with gateway_fixture(binary, project, env, case, args.ssh) as (gateway, url):
            browser = pw.chromium.launch(executable_path=args.chrome, headless=True, args=['--no-sandbox'])
            try:
                page = browser.new_page(viewport={'width': 1100, 'height': 650})
                page.on('dialog', lambda dialog: dialog.accept())
                page.goto(url)
                click_text(page, 'Send')
                visible(page, 'browser-stream-event')
                assert hits == ['/stream'], hits
                page.keyboard.press('Alt+1')
                click_text(page, 'note.txt')
                visible(page, 'stream note')
                page.evaluate("term.paste('responsive ')")
                page.keyboard.press('Control+s')
                until(lambda: note.read_text() == 'responsive stream note\n', 'typing/save during browser SSE')
                pids = child_pids(gateway.pid)
                assert len(pids) == 1
                page.close()
                until(lambda: not Path(f'/proc/{pids[0]}').exists(), 'SSE browser child reaped')
                until(disconnected.is_set, 'SSE socket closed on browser disconnect')
                assert hits == ['/stream']
                cases.append({'case': 'browser-sse-responsive-file-edit-and-disconnect-cancellation', 'status': 'passed', 'child_reaped': True, 'stream_closed': True})
            finally:
                browser.close()
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


def qualify_sql(pw, args, binary, base, environment, cases):
    helper = binary.parent / 'jdbc/kode-jdbc-bridge.jar' if args.package else ROOT / 'jdbc-bridge/target/kode-jdbc-bridge.jar'
    h2 = Path('/tmp/kode-rs-jdbc-maven-repository/com/h2database/h2/2.2.224/h2-2.2.224.jar')
    java = shutil.which('java')
    assert helper.is_file() and (args.package or h2.is_file()) and java, 'Build the JDBC/H2 qualification fixture first'
    case = base / 'sql'
    project = case / 'workspace'
    project.mkdir(parents=True)
    db = case / 'database'
    db.mkdir()
    definition = {'id': 'fixture', 'name': 'Browser SQL', 'driver': {'id': 'h2', 'driverClass': 'org.h2.Driver', 'jarPath': str(h2), 'artifact': {'repositoryType': 'LOCAL_FILE'}}, 'jdbcUrl': 'jdbc:h2:mem:browser_sql', 'username': 'sa', 'autoCommit': False}
    if args.package:
        definition['driver'] = {'id':'h2', 'driverClass':'org.h2.Driver'}
    (db / 'datasources-v1.json').write_text(json.dumps({'version': 1, 'dataSources': [definition]}))
    pid_path = case / 'java.pid'
    wrapper = case / 'java-fixture'
    wrapper.write_text(f'#!{sys.executable}\nimport os,sys\nfrom pathlib import Path\nPath({str(pid_path)!r}).write_text(str(os.getpid()))\nos.execv({java!r},[{java!r},*sys.argv[1:]])\n')
    wrapper.chmod(0o700)
    env = dict(environment, KODE_JAVA=str(wrapper), KODE_RS_DATABASE_HOME=str(db), KODE_RS_JDBC_HOME=str(case / 'plugins'))
    if args.package:
        env.pop('KODE_JDBC_HELPER', None)
    else:
        env['KODE_JDBC_HELPER'] = str(helper)
    with gateway_fixture(binary, project, env, case, args.ssh) as (gateway, url):
        browser = pw.chromium.launch(executable_path=args.chrome, headless=True, args=['--no-sandbox'])
        try:
            page = browser.new_page(viewport={'width': 1100, 'height': 650})
            page.on('dialog', lambda dialog: dialog.accept())
            page.goto(url)
            visible(page, 'Select a file to begin editing.')
            page.keyboard.press('Alt+4')
            visible(page, 'Browser SQL')
            page.keyboard.press('c')
            visible(page, 'BROWSER_SQL')
            page.keyboard.press('s')
            visible(page, 'Execute')
            page.keyboard.press('Control+a')
            page.evaluate("term.paste(\"select 'browser-sql-result';\")")
            # xterm's classic keyboard encoding sends Ctrl+Enter as plain CR.
            # Use the existing Execute button, as documented for such terminals.
            click_text(page, 'Execute')
            visible(page, 'Completed')
            visible(page, 'browser-sql-result')
            page.keyboard.press('Control+a')
            page.evaluate("term.paste('-- browser SQL dirty buffer\\n')")
            visible(page, 'browser SQL dirty buffer')
            until(pid_path.exists, 'actual Java helper started')
            java_pid = int(pid_path.read_text())
            pids = child_pids(gateway.pid)
            assert len(pids) == 1 and Path(f'/proc/{java_pid}').exists()
            page.close()
            until(lambda: not Path(f'/proc/{pids[0]}').exists(), 'SQL browser child reaped')
            until(lambda: not Path(f'/proc/{java_pid}').exists(), 'Java helper reaped on browser disconnect')
            session = project / '.kode-rs.json'
            until(lambda: session.exists() and 'browser SQL dirty buffer' in session.read_text(), 'SQL buffer persisted on disconnect')
            assert not (db / 'credentials-v1.json').exists()
            cases.append({'case': 'browser-actual-h2-sql-and-dirty-disconnect', 'status': 'passed', 'child_reaped': True, 'java_reaped': True, 'dirty_sql_preserved': True})
        except Exception:
            try:
                print(page.evaluate(SCREEN))
                print((case / 'gateway.log').read_text())
            except Exception:
                pass
            raise
        finally:
            browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ttyd', default=os.environ.get('KODE_TTYD'))
    parser.add_argument('--appimage', type=Path, help='Execute this actual AppImage using the matching package as the fixture inventory')
    parser.add_argument('--package', type=Path, help='Qualify a complete extracted Linux package with its adjacent ttyd/helper')
    parser.add_argument('--chrome', default=shutil.which('google-chrome') or shutil.which('chromium'))
    parser.add_argument('--ssh', action='store_true', help='Use isolated local OpenSSH servers and real -L tunnels for browser traffic')
    args = parser.parse_args()
    if args.appimage:
        assert args.package, '--appimage requires the matching extracted --package inventory'
        metadata=json.loads(args.appimage.with_name(args.appimage.name+'.manifest.json').read_text())
        assert metadata['payloadManifestSha256']==__import__('hashlib').sha256((args.package / 'package-manifest.json').read_bytes()).hexdigest()
    assert (args.ttyd or args.package) and args.chrome, 'Provide a package or real ttyd executable, and Chrome'
    cases = []
    with tempfile.TemporaryDirectory(prefix='kode-serve-browser-') as directory, sync_playwright() as pw:
        base = Path(directory)
        project = base / 'workspace with spaces'
        project.mkdir()
        file = project / 'code.rs'
        file.write_text('original\n')
        bin_dir = base / 'bin with spaces'
        bin_dir.mkdir()
        binary = bin_dir / 'kode'
        if args.package:
            shutil.copytree(args.package, bin_dir, dirs_exist_ok=True)
            if args.appimage:
                shutil.copy2(args.appimage, binary)
        else:
            try:
                os.link(ROOT / 'target/debug/kode', binary)
            except OSError:
                shutil.copy2(ROOT / 'target/debug/kode', binary)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        catalog = base / 'lsp.json'
        catalog.write_text('{"servers":[]}')
        env = dict(os.environ, XDG_CONFIG_HOME=str(base / 'config'),
                   KODE_RS_HOME=str(base / 'resources'), KODE_HOME=str(base / 'legacy-resources'),
                   KODE_LSP_CATALOG=str(catalog), KODE_LSP_HOME=str(base / 'lsp'),
                   KODE_RS_DATABASE_HOME=str(base / 'database'), KODE_RS_JDBC_HOME=str(base / 'plugins'),
                   KODE_JAVA=str(base / 'absent-java'))
        if args.package:
            env.pop('KODE_TTYD', None)
            env.pop('KODE_JDBC_HELPER', None)
            env.pop('LD_LIBRARY_PATH', None)
        else:
            env['KODE_TTYD'] = str(Path(args.ttyd).resolve())
        for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'KODE_CODEINTEL_PATH']:
            env.pop(key, None)
        credential = 'fixture:synthetic-password'
        log_path = base / 'gateway.log'
        with log_path.open('wb') as log, ExitStack() as stack:
            gateway = subprocess.Popen([str(binary), 'serve', '--port', str(port), '--credential', credential, str(project)], env=env, stdout=log, stderr=log)
            browser = None
            try:
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                url = f'http://127.0.0.1:{port}'

                def unauthorized():
                    try:
                        opener.open(url, timeout=.2)
                    except urllib.error.HTTPError as error:
                        return error.code == 401
                    except OSError:
                        return False
                    return False

                until(unauthorized, 'Basic authentication challenge')
                forwarded = stack.enter_context(ssh_forward(base, port, args.ssh))
                url = f'http://127.0.0.1:{forwarded}'
                until(unauthorized, 'forwarded Basic authentication challenge')
                browser = pw.chromium.launch(executable_path=args.chrome, headless=True, args=['--no-sandbox'])
                context = browser.new_context(viewport={'width': 1100, 'height': 650},
                    http_credentials={'username': 'fixture', 'password': 'synthetic-password'},
                    permissions=['clipboard-read', 'clipboard-write'])
                page = context.new_page()
                page.on('dialog', lambda dialog: dialog.accept())
                page.goto(url)
                page.wait_for_function('window.term && term.rows > 0')
                visible(page, 'Select a file to begin editing.')
                page.keyboard.press('Enter')
                click_text(page, 'code.rs')
                visible(page, 'original')
                # Browser clipboard -> xterm's actual bracketed-paste path.
                page.evaluate("navigator.clipboard.writeText('browser ')")
                page.evaluate('async () => term.paste(await navigator.clipboard.readText())')
                visible(page, 'browser original')
                page.keyboard.press('Control+s')
                until(lambda: file.read_text() == 'browser original\n', 'physical browser save')
                # B007: actual GUI key events traverse ttyd/xterm at both widths.
                panel_targets=[('Files','code.rs'),('Project','Project Settings'),('Git','[r] [b] [t] [T]'),('Database','Databases'),('REST','REST API'),('About','Kode '),('Settings','LSP Servers'),('Help','Kode shortcuts')]
                panel_sizes=[]
                for size in [{'width':1100,'height':650},{'width':640,'height':420}]:
                    page.set_viewport_size(size)
                    visible(page,'browser original')
                    for number,(title,marker) in enumerate(panel_targets,1):
                        click_text(page,'browser original')
                        page.keyboard.press(f'Alt+{number}')
                        page.wait_for_function("data => window.term && term.buffer.active.getLine(term.buffer.active.viewportY).translateToString(true).slice(0,36).includes(data[0]) && Array.from({length:term.rows-2},(_,i)=>term.buffer.active.getLine(i+1+term.buffer.active.viewportY)?.translateToString(true).slice(0,36)||'').join('\\n').includes(data[1])",arg=[title,marker],timeout=10000)
                    panel_sizes.append(page.evaluate('[term.cols,term.rows]'))
                assert file.read_text()=='browser original\n'
                cases.append({'case':'browser-all-eight-panel-shortcuts-visual-order','status':'passed','terminal_sizes':panel_sizes,'editor_unchanged':True})
                # Restore actual editor focus and the baseline clipboard caret.
                click_text(page,'browser original')
                page.keyboard.press('Home')
                for _ in range(len('browser ')):
                    page.keyboard.press('ArrowRight')
                page.wait_for_function("() => { const y=Array.from({length:term.rows},(_,i)=>term.buffer.active.getLine(i+term.buffer.active.viewportY)?.translateToString(true)||'').findIndex(line=>line.includes('browser original')); const line=term.buffer.active.getLine(y+term.buffer.active.viewportY).translateToString(true); return term.buffer.active.cursorY===y && term.buffer.active.cursorX===line.indexOf('browser original')+7; }",timeout=10000)
                page.set_viewport_size({'width':1100,'height':650})
                visible(page,'browser original')
                # xterm selection -> its existing browser copy listener.
                page.evaluate("navigator.clipboard.writeText('sentinel')")
                # Resize can clear xterm between a visible-text wait and the
                # selection. Test and select the actual row in one browser task.
                until(lambda: page.evaluate("""() => {
                    const lines = Array.from({length:term.rows},(_,i)=>term.buffer.active.getLine(i+term.buffer.active.viewportY)?.translateToString(true)||'');
                    const y = lines.findIndex(line=>line.includes('browser original'));
                    if (y < 0) return false;
                    term.select(lines[y].indexOf('browser original'), y+term.buffer.active.viewportY, 16);
                    return true;
                }"""), 'rendered editor row after resize')
                until(lambda: page.evaluate('navigator.clipboard.readText()') == 'browser original', 'xterm browser copy')
                page.evaluate('term.clearSelection(); term.focus()')
                page.set_viewport_size({'width': 640, 'height': 360})
                visible(page, 'browser original')
                page.evaluate("term.paste('dirty ')")
                visible(page, 'dirty original')
                pids = child_pids(gateway.pid)
                assert len(pids) == 1, pids
                page.close()
                until(lambda: not Path(f'/proc/{pids[0]}').exists(), 'browser child reaped after SIGHUP')
                session = project / '.kode-rs.json'
                until(lambda: session.exists() and 'browser dirty original' in session.read_text(), 'dirty session persisted on disconnect')
                assert file.read_text() == 'browser original\n', 'disconnect silently saved the dirty file'
                cases.append({'case': 'real-browser-auth-mouse-clipboard-edit-save-resize-disconnect',
                    'status': 'passed', 'child_reaped': True, 'dirty_session_persisted': True,
                    'core_without_java': True, 'chrome': browser.version})
                # The same live gateway restores the dirty buffer; no extra replay.
                page = context.new_page()
                page.on('dialog', lambda dialog: dialog.accept())
                page.goto(url)
                visible(page, 'browser dirty original')
                pids = child_pids(gateway.pid)
                page.close()
                until(lambda: all(not Path(f'/proc/{pid}').exists() for pid in pids), 'restored browser child reaped')
                cases.append({'case': 'browser-reconnect-restores-dirty-editor', 'status': 'passed'})
            except Exception:
                if browser and browser.contexts and browser.contexts[0].pages:
                    page = browser.contexts[0].pages[0]
                    try:
                        print(page.evaluate(SCREEN))
                    except Exception:
                        pass
                print(log_path.read_text())
                raise
            finally:
                if browser:
                    browser.close()
                gateway.terminate()
                gateway.wait(timeout=10)
            output = log_path.read_text()
            assert credential not in output
            assert base64.b64encode(credential.encode()).decode() not in output, 'ttyd leaked its Basic auth token'
        qualify_stream(pw, args, binary, base, env, cases)
        qualify_sql(pw, args, binary, base, env, cases)
    if args.package:
        (ROOT / ('docs/parity/Z03-appimage-browser.json' if args.appimage else 'docs/parity/Z03-browser.json')).write_text(json.dumps({'appimage_sha256':__import__('hashlib').sha256(args.appimage.read_bytes()).hexdigest() if args.appimage else None,'package_manifest_sha256':__import__('hashlib').sha256((args.package / 'package-manifest.json').read_bytes()).hexdigest(),'transport':'ssh-forwarded-loopback' if args.ssh else 'loopback','packaged':True,'adjacent_ttyd':True,'adjacent_helper':True,'developer_library_path':False,'cases':cases,'passed':len(cases)},indent=2)+'\n')
    else:
        (ROOT / 'docs/parity/Z01-browser.json').write_text(json.dumps({'transport': 'ssh-forwarded-loopback' if args.ssh else 'loopback', 'cases': cases, 'passed': len(cases)}, indent=2) + '\n')
        panel_cases=[case for case in cases if case['case']=='browser-all-eight-panel-shortcuts-visual-order']
        (ROOT / 'docs/parity/U10-browser.json').write_text(json.dumps({'transport':'ssh-forwarded-loopback' if args.ssh else 'loopback','cases':panel_cases,'passed':len(panel_cases)},indent=2)+'\n')
    print(f'Passed {len(cases)} real ttyd/Chrome browser checks')


if __name__ == '__main__':
    main()
