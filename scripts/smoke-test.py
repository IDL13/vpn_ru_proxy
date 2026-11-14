"""Exercise the native helper through Chrome's framed JSON protocol."""

import http.client
import http.server
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import threading
import time
from contextlib import contextmanager


WINDOWS = sys.platform == "win32"
HOST = "org.vpn_ru_proxy.chrome"
EXTENSION = "a" * 32
ORIGIN = f"chrome-extension://{EXTENSION}/"


def register(helper, directory, env):
    if WINDOWS:
        base = Path(env["VPN_RU_PROXY_DATA_DIR"])
    elif sys.platform == "linux":
        env = env | {"XDG_CONFIG_HOME": str(directory / "config")}
        env.pop("CHROME_CONFIG_HOME", None)
        env.pop("CHROME_USER_DATA_DIR", None)
        base = Path(env["XDG_CONFIG_HOME"]) / "google-chrome"
    else:
        env = env | {"HOME": str(directory)}
        base = directory / "Library/Application Support/Google/Chrome"
    manifest = base / "NativeMessagingHosts" / f"{HOST}.json"
    if WINDOWS:
        import winreg

        key = rf"Software\Google\Chrome\NativeMessagingHosts\{HOST}"
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key):
                pass
        except FileNotFoundError:
            pass
        else:
            raise RuntimeError("Use a Windows test account without an existing VPN RU Proxy host.")
    try:
        subprocess.run([helper, "--install-chrome-host", EXTENSION], env=env, check=True, timeout=10)
        data = json.loads(manifest.read_text(encoding="utf-8"))
        assert Path(data["path"]) == helper, data
        assert data["allowed_origins"] == [ORIGIN], data
        if WINDOWS:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key) as handle:
                assert Path(winreg.QueryValue(handle, None)) == manifest
    finally:
        if WINDOWS and manifest.exists():
            try:
                winreg.DeleteKey(winreg.HKEY_CURRENT_USER, key)
            except FileNotFoundError:
                pass


@contextmanager
def native_host(helper, env):
    process = subprocess.Popen([helper, ORIGIN], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        yield process
        if process.poll() is None:
            process.stdin.close()
            assert process.wait(timeout=10) == 0
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        process.stdin.close()
        process.stdout.close()


def request(process, action, payload=None):
    message = {"action": action}
    if payload is not None:
        message["payload"] = payload
    body = json.dumps(message).encode()
    process.stdin.write(struct.pack("=I", len(body)) + body)
    process.stdin.flush()
    header = process.stdout.read(4)
    assert len(header) == 4, f"No response to {action}"
    return json.loads(process.stdout.read(struct.unpack("=I", header)[0]))


def expect_stopped():
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        connection = http.client.HTTPConnection("127.0.0.1", 17892, timeout=0.25)
        try:
            connection.connect()
        except ConnectionRefusedError:
            return
        finally:
            connection.close()
        time.sleep(0.05)
    raise AssertionError("Port 17892 is still open")


class LocalPage(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"VPN RU Proxy smoke test")


def check_proxy():
    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), LocalPage) as server:
        worker = threading.Thread(target=server.serve_forever)
        worker.start()
        try:
            connection = http.client.HTTPConnection("127.0.0.1", 17892, timeout=5)
            try:
                connection.request("GET", f"http://127.0.0.1:{server.server_port}/")
                response = connection.getresponse()
                assert response.status == 200 and response.read() == b"VPN RU Proxy smoke test"
            finally:
                connection.close()
        finally:
            server.shutdown()
            worker.join()


def main():
    helper = Path(sys.argv[1]).resolve()
    name = "sing-box.exe" if WINDOWS else "sing-box"
    sing_box = next(Path(sys.argv[2]).resolve().rglob(name))
    config_only = "--config-only" in sys.argv[3:]
    interface = os.environ.get("TEST_INTERFACE")
    if WINDOWS and not interface:
        interface = subprocess.check_output([
            "powershell.exe", "-NoProfile", "-Command",
            "[Console]::OutputEncoding = [Text.Encoding]::UTF8; "
            "(Get-NetIPAddress -IPAddress 127.0.0.1).InterfaceAlias",
        ], encoding="utf-8", timeout=10).strip()
    assert interface

    with tempfile.TemporaryDirectory(prefix="VPN RU Proxy ") as directory:
        directory = Path(directory)
        data = directory / "settings"
        env = os.environ | {"VPN_RU_PROXY_DATA_DIR": str(data), "SING_BOX_BIN": str(sing_box)}
        register(helper, directory, env)
        if not config_only:
            expect_stopped()
        with native_host(helper, env) as process:
            assert request(process, "status") == {"running": False}
            assert request(process, "settings")["settings"]["sites"] == []
            assert "error" in request(process, "save", {"interface": interface, "sites": "", "dns_server": "invalid"})
            edit = {"interface": interface, "sites": "https://www.example.com/test", "dns_server": "192.0.2.53"}
            saved = request(process, "save", edit)
            assert saved == {"settings": {"interface": interface, "sites": ["example.com"], "dns_server": "192.0.2.53"}}, saved
            assert request(process, "settings") == saved
            assert request(process, "import", [{"hostname": "*.example.org", "ip": "2001:db8::1"}]) == {"sites": ["2001:db8::1", "example.org"]}

            started = request(process, "start")
            config = json.loads((data / "sing-box.json").read_text(encoding="utf-8"))
            assert config["dns"]["servers"][1] == {
                "type": "udp", "tag": "physical-dns", "server": "192.0.2.53", "bind_interface": interface,
            }
            assert config["outbounds"][1]["bind_interface"] == interface
            assert config["route"]["rules"][0]["domain"] == ["example.com"]
            if config_only:
                assert started.get("running") or "Порт прокси" in started.get("error", ""), started
                print("Chrome native registration, settings, import and sing-box config validation passed.")
                return

            assert started == {"running": True}, started
            check_proxy()
            updated = request(process, "save", edit | {"sites": "example.net", "dns_server": ""})
            assert updated["settings"]["sites"] == ["example.net"], updated
            assert "dns_server" not in updated["settings"], updated
            assert request(process, "status") == {"running": True}
            assert request(process, "stop") == {"running": False}
            expect_stopped()
            assert request(process, "start") == {"running": True}
            if WINDOWS or sys.platform == "linux":
                process.kill()
                process.wait(timeout=10)
        expect_stopped()
    print("Chrome native registration, settings, import, proxy startup, HTTP forwarding and cleanup passed.")


if __name__ == "__main__":
    main()
