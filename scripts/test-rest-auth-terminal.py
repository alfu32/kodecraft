#!/usr/bin/env python3
"""Select, edit and send every M002 auth choice in actual isolated CLI PTYs."""
import base64
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import re
import struct
import tempfile
import threading
from urllib.parse import parse_qsl, quote, urlsplit, unquote

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("editors", ROOT / "scripts/test-rest-editor-terminal.py")
editors = importlib.util.module_from_spec(spec)
spec.loader.exec_module(editors)

CHOICES = [
    ("noauth", "No auth", {}, None),
    ("apikey", "API key", {"key": "X-API-Key", "value": "synthetic-key", "in": "header"}, "value"),
    ("bearer", "Bearer", {"token": "synthetic-token"}, "token"),
    ("basic", "Basic", {"username": "User", "password": "Password"}, "password"),
    ("digest", "Digest", {"username": "User", "password": "Password"}, "password"),
    ("oauth1", "OAuth 1.0", {"consumerKey": "consumer", "consumerSecret": "secret", "token": "token", "tokenSecret": "token-secret", "signatureMethod": "HMAC-SHA1", "realm": ""}, "consumerSecret"),
    ("oauth2", "OAuth 2.0", {"accessToken": "synthetic-token", "headerPrefix": "Bearer"}, "accessToken"),
    ("hawk", "Hawk", {"authId": "fixture", "authKey": "synthetic-key", "algorithm": "sha256", "extraData": "", "app": "", "dlg": ""}, "authKey"),
    ("awsv4", "AWS Signature", {"accessKey": "AKIDEXAMPLE", "secretKey": "synthetic-secret", "region": "us-east-1", "service": "execute-api", "sessionToken": ""}, "secretKey"),
    ("ntlm", "NTLM", {"username": "User", "password": "Password", "domain": "DOMAIN", "workstation": "fixture"}, "password"),
    ("edgegrid", "Akamai EdgeGrid", {"clientToken": "client", "clientSecret": "synthetic-secret", "accessToken": "access", "headersToSign": "", "maxBody": "131072"}, "clientSecret"),
]


def parameters(value):
    return dict((k, unquote(v)) for k, v in re.findall(r'(\w+)="([^"]*)"', value))


def b64(data):
    return base64.b64encode(data).decode()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def verify(kind, header, headers, path, body):
    if kind == "noauth":
        assert header is None
    elif kind == "apikey":
        assert headers["X-API-Key"] == "synthetic-key"
    elif kind == "bearer" or kind == "oauth2":
        assert header == "Bearer synthetic-token"
    elif kind == "basic":
        assert header == "Basic " + b64(b"User:Password")
    elif kind == "digest":
        fields = parameters(header)
        fields.update(dict(re.findall(r'\b(qop|nc)=([^, ]+)', header)))
        md5 = lambda value: hashlib.md5(value.encode()).hexdigest()
        expected = md5(f'{md5("User:fixture:Password")}:server-nonce:{fields["nc"]}:{fields["cnonce"]}:auth:{md5("POST:"+path)}')
        assert fields["response"] == expected
    elif kind == "oauth1":
        fields = parameters(header)
        supplied = fields.pop("oauth_signature")
        encode = lambda text: quote(text, safe="~-._")
        values = sorted((encode(k), encode(v)) for k, v in list(fields.items()) + parse_qsl(urlsplit(path).query, keep_blank_values=True))
        normalized = "&".join(k + "=" + v for k, v in values)
        base_url = "http://" + headers["Host"] + urlsplit(path).path
        base = "POST&" + encode(base_url) + "&" + encode(normalized)
        assert supplied == b64(hmac.new(b"secret&token-secret", base.encode(), hashlib.sha1).digest())
    elif kind == "hawk":
        fields = parameters(header)
        payload = b64(hashlib.sha256(b"hawk.1.payload\ntext/plain\n" + body + b"\n").digest())
        assert fields["hash"] == payload
        host, port = headers["Host"].split(":")
        normalized = f'hawk.1.header\n{fields["ts"]}\n{fields["nonce"]}\nPOST\n{path}\n{host}\n{port}\n{payload}\n\n'
        assert fields["mac"] == b64(hmac.new(b"synthetic-key", normalized.encode(), hashlib.sha256).digest())
    elif kind == "awsv4":
        credential, names, signature = re.fullmatch(r'AWS4-HMAC-SHA256 Credential=([^,]+), SignedHeaders=([^,]+), Signature=(\w+)', header).groups()
        access, date, region, service, end = credential.split("/")
        assert (access, region, service, end) == ("AKIDEXAMPLE", "us-east-1", "execute-api", "aws4_request")
        canonical_headers = "".join(name + ":" + " ".join(headers[name].split()) + "\n" for name in names.split(";"))
        parsed = urlsplit(path)
        canonical = f'POST\n{parsed.path}\n{parsed.query}\n{canonical_headers}\n{names}\n{sha(body)}'
        scope = "/".join([date, region, service, end])
        string = f'AWS4-HMAC-SHA256\n{headers["X-Amz-Date"]}\n{scope}\n{sha(canonical.encode())}'
        key = b"AWS4synthetic-secret"
        for value in [date, region, service, end]:
            key = hmac.new(key, value.encode(), hashlib.sha256).digest()
        assert signature == hmac.new(key, string.encode(), hashlib.sha256).hexdigest()
    elif kind == "ntlm":
        message = base64.b64decode(header.removeprefix("NTLM "))
        assert message[:12] == b"NTLMSSP\0\x03\0\0\0"
        length, _, offset = struct.unpack_from("<HHI", message, 20)
        response = message[offset:offset + length]
        # Public Microsoft example: NT hash of Password. Validate the NTLMv2
        # proof independently of the Rust client's random timestamp/nonce.
        nt_hash = bytes.fromhex("a4f49c406510bdcab6824ee7c30fd852")
        key = hmac.new(nt_hash, "USERDOMAIN".encode("utf-16-le"), hashlib.md5).digest()
        assert response[:16] == hmac.new(key, bytes(range(1, 9)) + response[16:], hashlib.md5).digest()
    elif kind == "edgegrid":
        fields = dict(part.split("=", 1) for part in header.removeprefix("EG1-HMAC-SHA256 ").split(";"))
        unsigned = header[:header.index("signature=")]
        key = b64(hmac.new(b"synthetic-secret", fields["timestamp"].encode(), hashlib.sha256).digest())
        data = "\t".join(["POST", "http", headers["Host"], path, "", b64(hashlib.sha256(body).digest()), unsigned])
        assert fields["signature"] == b64(hmac.new(key.encode(), data.encode(), hashlib.sha256).digest())


def main():
    hits, failures, cases = [], [], []

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            kind = self.path.split("/")[-1]
            header = self.headers.get("Authorization")
            challenge = None
            if kind == "digest" and header is None:
                challenge = 'Digest realm="fixture", nonce="server-nonce", algorithm=MD5, qop="auth"'
            elif kind == "ntlm" and base64.b64decode(header.removeprefix("NTLM "))[8:12] == b"\x01\0\0\0":
                empty = struct.pack("<HHI", 0, 0, 56)
                message = b"NTLMSSP\0" + struct.pack("<I", 2) + empty + struct.pack("<I", 0x00800201) + bytes(range(1, 9)) + bytes(8) + empty + bytes(8)
                challenge = "NTLM " + b64(message)
            if challenge:
                self.send_response(401)
                self.send_header("WWW-Authenticate", challenge)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            try:
                assert body == b"auth-body"
                verify(kind, header, self.headers, self.path, body)
                hits.append(kind)
                response, status = ("AUTH_OK_" + kind).encode(), 200
            except Exception as error:
                failures.append((kind, type(error).__name__))
                response, status = b"AUTH_FIXTURE_REJECTED", 401
            self.send_response(status)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(response)))
            self.end_headers()
            self.wfile.write(response)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix="kode-rest-auth-") as directory:
            base = Path(directory)
            catalog = base / "lsp.json"
            catalog.write_text('{"servers":[]}')
            for width in [100, 60]:
                for kind, label, values, secret in CHOICES:
                    project = base / f"{width}-{kind}"
                    project.mkdir()
                    # Retain inactive scheme fields while changing the selection
                    # through the actual form. No credentials come from the host.
                    definition = {name: [{"key": key, "value": value, "type": "string"} for key, value in fields.items()] for name, _, fields, _ in CHOICES}
                    if secret:
                        next(entry for entry in definition[kind] if entry["key"] == secret)["value"] = "STALE-WILL-BE-EDITED"
                    definition["type"] = "noauth" if kind != "noauth" else "bearer"
                    collection = {"info": {"name": "Auth fixture"}, "x-unknown": 42, "item": [{"name": "Auth request", "request": {"method": "POST", "url": f"http://127.0.0.1:{server.server_port}/{kind}", "auth": definition, "body": {"mode": "raw", "raw": "auth-body"}}}]}
                    state = {"version": 1, "restApi": {"collection": collection}, "activeOrigin": {"Virtual": {"owner": "rest-node", "key": "0", "label": "REST request"}}}
                    (project / ".kode-rs.json").write_text(json.dumps(state))
                    actions = [(b"REQUEST | Auth", .15, editors.click("Auth", first=False)), (b"Auth:", .15, editors.click(label))]
                    if secret:
                        actions += [(f"Auth: {kind}".encode(), .15, editors.click(secret + " =")), (b"Authentication value", .15, b"\x01" + editors.paste(values[secret]) + b"\r")]
                    actions += [(f"Auth: {kind}".encode(), .15, editors.click("Send")), (("AUTH_OK_" + kind).encode(), .15, b"\x11")]
                    env = {"KODE_LSP_CATALOG": str(catalog), "KODE_LSP_HOME": str(base / "no-lsp"), "KODE_RS_DATABASE_HOME": str(project / "db"), "KODE_RS_JDBC_HOME": str(project / "jdbc"), "KODE_JAVA": str(base / "absent-java")}
                    result = editors.terminal.run_case(project, base / "config", (width, 40), actions, env_overrides=env)
                    saved = json.loads((project / ".kode-rs.json").read_text())["restApi"]["collection"]
                    assert saved["item"][0]["request"]["auth"]["type"] == kind
                    assert {entry["key"]: entry["value"] for entry in saved["item"][0]["request"]["auth"][kind]} == values
                    assert saved["x-unknown"] == 42
                    assert not failures, failures
                    cases.append({"case": "select-edit-sign-send-" + kind, **result})
                    print(f"Passed {kind} at {width} columns", flush=True)
        assert len(hits) == 22, hits
    finally:
        server.shutdown()
        server.server_close()
    (ROOT / "docs/parity/R09-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} real authentication PTYs")


if __name__ == "__main__":
    main()
