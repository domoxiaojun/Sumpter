# /// script
# requires-python = ">=3.11"
# dependencies = ["aiohttp==3.14.5"]
# ///
"""Loopback-only gateway contracts; never use real login or proxy credentials."""
# unittest owns asyncSetUp/asyncTearDown; fixture fields are initialized there.
# pyright: reportUninitializedInstanceVariable=false

from __future__ import annotations

import asyncio
import gzip
import importlib.util
import json
import ssl
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

import aiohttp
from aiohttp import web
from aiohttp.test_utils import TestServer
from yarl import URL

SOURCE = Path(__file__).parents[1] / "clients/codex_desktop_gateway.py"
spec = importlib.util.spec_from_file_location("codex_desktop_gateway", SOURCE)
assert spec and spec.loader
gateway = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = gateway
spec.loader.exec_module(gateway)


class GatewayContract(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.requests = []
        self.created_calls = set()
        self.discovery: dict[str, Any] = {
            "accounts": [{"id": "synthetic-account", "workspace_backend_origin": "NO_CONSTRAINT",
                          "account_routing_override": "NO_CONSTRAINT", "plan": "unchanged"}],
            "untouched": {"permission": False},
        }
        backend = web.Application()
        backend.router.add_route("*", "/{path:.*}", self.backend_handler)
        self.backend = TestServer(backend)
        await self.backend.start_server()
        sumpter = web.Application()
        sumpter.router.add_route("*", "/{path:.*}", self.sumpter_handler)
        self.sumpter = TestServer(sumpter)
        await self.sumpter.start_server()
        self.config = self.root / "config.toml"
        self.auth = self.root / "auth.json"
        self.config.write_text(
            'model_provider = "sumpter"\n[model_providers.sumpter]\n'
            f'base_url = "{self.sumpter.make_url("/v1")}"\n'
            'experimental_bearer_token = "synthetic-sumpter-token"\n'
        )
        self.write_auth("synthetic-chatgpt-token")
        self.settings = gateway.Settings(self.config, self.auth)
        self.app = gateway.create_app(self.settings, backend=str(self.backend.make_url("")))
        self.server = TestServer(self.app)
        await self.server.start_server()
        self.client = aiohttp.ClientSession(auto_decompress=False)
        self.headers = {"Authorization": "Bearer synthetic-chatgpt-token",
                        "ChatGPT-Account-Id": "synthetic-account"}

    def write_auth(self, token):
        self.auth.write_text(json.dumps({"tokens": {"access_token": token, "account_id": "synthetic-account"}}))

    async def asyncTearDown(self):
        await self.client.close()
        await self.server.close()
        await self.sumpter.close()
        await self.backend.close()
        self.temp.cleanup()

    async def backend_handler(self, request):
        self.requests.append(("backend", request.raw_path, dict(request.headers), await request.read()))
        if request.path == gateway.DISCOVERY:
            return web.json_response(self.discovery)
        if request.path == "/backend-api/redirect-ws":
            raise web.HTTPFound(location=str(self.sumpter.make_url("/must-not-receive-login")))
        if request.headers.get("Upgrade", "").lower() == "websocket":
            ws = web.WebSocketResponse(protocols=["synthetic-protocol"], autoping=False)
            await ws.prepare(request)
            while True:
                message = await ws.receive()
                if message.type == aiohttp.WSMsgType.TEXT:
                    await ws.send_str(message.data)
                elif message.type == aiohttp.WSMsgType.BINARY:
                    await ws.send_bytes(message.data)
                elif message.type == aiohttp.WSMsgType.PING:
                    await ws.pong(message.data)
                else:
                    await ws.close()
                    break
            return ws
        if request.path == "/backend-api/compressed":
            return web.Response(body=gzip.compress(b"unchanged compressed response", mtime=0),
                                headers={"Content-Encoding": "gzip"})
        if request.path == "/backend-api/error":
            return web.Response(status=429, body=b'{"error":"synthetic quota"}', headers={"Retry-After": "17"})
        if request.path == "/backend-api/stream":
            response = web.StreamResponse(headers={"Content-Type": "text/event-stream"})
            await response.prepare(request)
            await response.write(b"data: first\n\n")
            await asyncio.sleep(0.01)
            await response.write(b"data: second\n\n")
            return response
        if request.path == "/backend-api/truncated":
            response = web.StreamResponse(headers={"Content-Length": "100"})
            await response.prepare(request)
            await response.write(b"partial")
            if request.transport:
                request.transport.close()
            return response
        return web.Response(body=b"official fixture")

    async def sumpter_handler(self, request):
        body = await request.read()
        self.requests.append(("sumpter", request.raw_path, dict(request.headers), body))
        if request.headers.get("Authorization") != "Bearer synthetic-sumpter-token":
            raise web.HTTPUnauthorized()
        if request.path == "/v1/realtime/calls" and request.method == "POST":
            self.created_calls.add("rtc_synthetic")
            return web.Response(status=201, body=b"v=synthetic-answer\r\n",
                                headers={"Location": "/v1/realtime/calls/rtc_synthetic"})
        if request.path == "/v1/live/rtc_synthetic" and "rtc_synthetic" in self.created_calls:
            ws = web.WebSocketResponse()
            await ws.prepare(request)
            await ws.send_str('{"type":"session.created"}')
            await ws.close()
            return ws
        raise web.HTTPNotFound()

    async def test_voice_creation_then_existing_sideband_uses_same_call(self):
        payload = b'{"sdp":"v=offer\\r\\n","session":{"model":"gpt-live-1.1-codex","delegation":{"type":"client"},"voice_session_id":"synthetic-voice"}}'
        headers = {**self.headers, "Content-Type": "application/json", "Cookie": "private-cookie",
                   "X-OAI-Attestation": "private-attestation", "OpenAI-Alpha": "quicksilver=v2"}
        path = gateway.CALLS + "?intent=quicksilver&architecture=avas"
        async with self.client.post(self.server.make_url(path), data=payload, headers=headers) as response:
            self.assertEqual(response.status, 201)
            self.assertEqual(await response.read(), b"v=synthetic-answer\r\n")
            self.assertEqual(response.headers["Location"], "/v1/realtime/calls/rtc_synthetic")
        forwarded = next(r for r in self.requests if r[0] == "sumpter")
        self.assertEqual(forwarded[1], "/v1/realtime/calls?intent=quicksilver&architecture=avas")
        self.assertEqual(forwarded[3], payload)
        self.assertEqual(forwarded[2]["Authorization"], "Bearer synthetic-sumpter-token")
        self.assertNotIn("ChatGPT-Account-Id", forwarded[2])
        self.assertNotIn("Cookie", forwarded[2])
        self.assertNotIn("X-OAI-Attestation", forwarded[2])
        async with self.client.ws_connect(self.sumpter.make_url("/v1/live/rtc_synthetic"),
                                         headers={"Authorization": "Bearer synthetic-sumpter-token"}) as ws:
            self.assertEqual((await ws.receive()).data, '{"type":"session.created"}')

    async def test_account_discovery_is_not_rewritten(self):
        async with self.client.get(self.server.make_url(gateway.DISCOVERY), headers=self.headers) as response:
            self.assertEqual(response.status, 200)
            self.assertEqual(await response.json(), self.discovery)
        self.assertTrue(all(r[2].get("Authorization") == self.headers["Authorization"] for r in self.requests))

    async def test_constrained_workspace_stops_before_voice(self):
        self.discovery["accounts"][0]["workspace_backend_origin"] = "https://chatgpt.com"
        async with self.client.post(self.server.make_url(gateway.CALLS), headers=self.headers) as response:
            self.assertEqual(response.status, 503)
        self.assertFalse(any(r[0] == "sumpter" for r in self.requests))

    async def test_login_rotation_rejects_old_token(self):
        self.write_auth("synthetic-new-token")
        async with self.client.get(self.server.make_url("/backend-api/example"), headers=self.headers) as response:
            self.assertEqual(response.status, 401)
        self.assertEqual(self.requests, [])

    async def test_nonvoice_http_preserves_body_query_headers_and_error(self):
        url = URL(str(self.server.make_url("")) + "/backend-api/example?q=a%2Fb", encoded=True)
        async with self.client.post(url,
                                    data=b"unchanged request", headers=self.headers) as response:
            self.assertEqual(await response.read(), b"official fixture")
        self.assertEqual(self.requests[-1][1], "/backend-api/example?q=a%2Fb")
        self.assertEqual(self.requests[-1][3], b"unchanged request")
        self.assertEqual(self.requests[-1][2]["Authorization"], self.headers["Authorization"])
        async with self.client.get(self.server.make_url("/backend-api/error"), headers=self.headers) as response:
            self.assertEqual(response.status, 429)
            self.assertEqual(response.headers["Retry-After"], "17")
            self.assertEqual(await response.read(), b'{"error":"synthetic quota"}')

    async def test_compression_and_stream_bytes_are_unchanged(self):
        async with self.client.get(self.server.make_url("/backend-api/compressed"), headers=self.headers) as response:
            self.assertEqual(response.headers["Content-Encoding"], "gzip")
            self.assertEqual(await response.read(), gzip.compress(b"unchanged compressed response", mtime=0))
        async with self.client.get(self.server.make_url("/backend-api/stream"), headers=self.headers) as response:
            self.assertEqual(await response.read(), b"data: first\n\ndata: second\n\n")

    async def test_backend_websocket_frames_and_protocol(self):
        async with self.client.ws_connect(self.server.make_url("/backend-api/celsius/ws/user"),
                                         headers=self.headers, protocols=["synthetic-protocol"], autoping=False) as ws:
            self.assertEqual(ws.protocol, "synthetic-protocol")
            await ws.send_str("hello")
            self.assertEqual((await ws.receive()).data, "hello")
            await ws.send_bytes(b"binary")
            self.assertEqual((await ws.receive()).data, b"binary")
            await ws.ping(b"ping")
            message = await ws.receive()
            self.assertEqual(message.type, aiohttp.WSMsgType.PONG)
            self.assertEqual(message.data, b"ping")

    async def test_truncated_upstream_is_not_reported_as_clean_eof(self):
        async with self.client.get(self.server.make_url("/backend-api/truncated"), headers=self.headers) as response:
            self.assertEqual(response.status, 200)
            with self.assertRaises(aiohttp.ClientPayloadError):
                await response.read()

    async def test_websocket_redirect_does_not_forward_login(self):
        with self.assertRaises(aiohttp.WSServerHandshakeError) as rejected:
            await self.client.ws_connect(self.server.make_url("/backend-api/redirect-ws"), headers=self.headers)
        self.assertEqual(rejected.exception.status, 503)
        self.assertFalse(any(r[0] == "sumpter" for r in self.requests))

    async def test_encoded_voice_route_and_foreign_account_are_rejected(self):
        target = URL(str(self.server.make_url("")) + "/backend-api/wham/realtime/%63alls", encoded=True)
        async with self.client.post(target, headers=self.headers) as response:
            self.assertEqual(response.status, 400)
        async with self.client.get(self.server.make_url("/backend-api/example"),
                                   headers={**self.headers, "ChatGPT-Account-Id": "foreign"}) as response:
            self.assertEqual(response.status, 403)
        self.assertFalse(any(r[0] == "sumpter" for r in self.requests))

    async def test_https_with_explicit_test_trust(self):
        cert, key = self.root / "test-cert.pem", self.root / "test-key.pem"
        await asyncio.to_thread(subprocess.run, ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                        "-keyout", str(key), "-out", str(cert), "-subj", "/CN=127.0.0.1",
                        "-addext", "subjectAltName=IP:127.0.0.1"],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        server_tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        server_tls.load_cert_chain(cert, key)
        test_app = gateway.create_app(self.settings, backend=str(self.backend.make_url("")))
        server = TestServer(test_app, scheme="https")
        await server.start_server(ssl=server_tls)
        try:
            client_tls = ssl.create_default_context(cafile=str(cert))
            async with self.client.get(server.make_url("/backend-api/example"), headers=self.headers,
                                       ssl=client_tls) as response:
                self.assertEqual(response.status, 200)
        finally:
            await server.close()


if __name__ == "__main__":
    unittest.main()
