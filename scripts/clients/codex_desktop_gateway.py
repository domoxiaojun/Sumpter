# /// script
# requires-python = ">=3.11"
# dependencies = ["aiohttp==3.14.5"]
# ///
"""Opt-in HTTPS desktop gateway prototype; no config, trust-store or app edits."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import hmac
import ipaddress
import json
import logging
import os
import ssl
import time
import tomllib
from collections.abc import AsyncIterator
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import urlsplit

import aiohttp
from aiohttp import web
from multidict import CIMultiDict
from yarl import URL

BACKEND = "https://chatgpt.com"
DISCOVERY = "/backend-api/wham/accounts/check"
CALLS = "/backend-api/wham/realtime/calls"
BODY_LIMIT = 128 * 1024 * 1024
DISCOVERY_LIMIT = 1024 * 1024
HOP_HEADERS = {
    "connection", "keep-alive", "proxy-authenticate", "proxy-authorization",
    "te", "trailer", "transfer-encoding", "upgrade", "host",
}
WS_HEADERS = {
    "sec-websocket-key", "sec-websocket-version", "sec-websocket-extensions",
    "sec-websocket-protocol", "sec-websocket-accept", "content-length",
}
VOICE_HEADERS = {
    "content-type", "accept", "openai-alpha", "thread-id", "session-id",
    "voice-session-id", "x-codex-turn-metadata", "user-agent", "originator",
}


class GatewayError(Exception):
    """Messages must never contain request data or credentials."""


@dataclass(frozen=True)
class Settings:
    config_file: Path
    auth_file: Path


@dataclass(frozen=True)
class Credentials:
    access_token: str
    account_id: str
    sumpter_url: str
    sumpter_token: str

    def identity(self) -> bytes:
        return hashlib.sha256(
            (self.account_id + "\0" + self.access_token).encode()
        ).digest()


def read_credentials(settings: Settings) -> Credentials:
    try:
        config = tomllib.loads(settings.config_file.read_text())
        auth = json.loads(settings.auth_file.read_text())
        provider = config["model_providers"][config["model_provider"]]
        base = provider["base_url"].rstrip("/")
        parsed = urlsplit(base)
        host = parsed.hostname or ""
        loopback = host == "localhost" or ipaddress.ip_address(host).is_loopback
        if (
            not loopback or parsed.scheme not in {"http", "https"}
            or parsed.username is not None or parsed.password is not None
            or parsed.query or parsed.fragment
        ):
            raise GatewayError("The selected provider must be a loopback Sumpter URL")
        token = (
            os.environ.get(provider["env_key"], "")
            if provider.get("env_key")
            else provider.get("experimental_bearer_token", "")
        )
        tokens = auth["tokens"]
        access, account = tokens["access_token"], tokens["account_id"]
        if not all(isinstance(v, str) and v and v.strip() == v
                   for v in (token, access, account)):
            raise GatewayError("Provider bearer token and ChatGPT login are required")
        return Credentials(access, account, base, token)
    except GatewayError:
        raise
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        raise GatewayError("Cannot read the selected provider or ChatGPT login") from None


def check_discovery(body: bytes, account_id: str) -> None:
    try:
        data = json.loads(body)
        accounts = data["accounts"]
        matches = [a for a in accounts if a["id"] == account_id]
        if len(matches) != 1:
            raise GatewayError("Account routing discovery must match one selected account")
        account = matches[0]
        if (
            account.get("workspace_backend_origin") != "NO_CONSTRAINT"
            or account.get("account_routing_override") != "NO_CONSTRAINT"
        ):
            raise GatewayError("This prototype does not support constrained workspace routing")
    except GatewayError:
        raise
    except (ValueError, TypeError, KeyError):
        raise GatewayError("Invalid account routing discovery response") from None


def clean_headers(headers: CIMultiDict[str], *, websocket: bool = False) -> CIMultiDict[str]:
    blocked = HOP_HEADERS | {
        part.strip().lower()
        for value in headers.getall("Connection", [])
        for part in value.split(",")
    }
    if websocket:
        blocked |= WS_HEADERS
    return CIMultiDict((k, v) for k, v in headers.items() if k.lower() not in blocked)


class Gateway:
    def __init__(self, settings: Settings, *, backend: str = BACKEND):
        self.settings = settings
        # Test injection only. There is deliberately no client or CLI upstream override.
        self.backend = backend.rstrip("/")
        self.session: aiohttp.ClientSession | None = None
        self.local_session: aiohttp.ClientSession | None = None
        self.checked_identity: bytes | None = None
        self.checked_at = 0.0
        self.routing_lock = asyncio.Lock()

    async def start(self, _app: web.Application) -> None:
        timeout = aiohttp.ClientTimeout(total=None, sock_connect=15, sock_read=180)
        trace = aiohttp.TraceConfig()

        async def reject_redirect(
            _session: aiohttp.ClientSession,
            _context: SimpleNamespace,
            _params: aiohttp.TraceRequestRedirectParams,
        ) -> None:
            raise GatewayError("Desktop gateway will not follow an upstream redirect")

        trace.on_request_redirect.append(reject_redirect)
        self.session = aiohttp.ClientSession(
            timeout=timeout, auto_decompress=False, cookie_jar=aiohttp.DummyCookieJar(),
            trust_env=True, trace_configs=[trace],
        )
        # Sumpter is loopback: never send its credential through an environment proxy.
        self.local_session = aiohttp.ClientSession(
            timeout=timeout, auto_decompress=False, cookie_jar=aiohttp.DummyCookieJar(),
            trust_env=False, trace_configs=[trace],
        )

    async def stop(self, _app: web.Application) -> None:
        for session in (self.session, self.local_session):
            if session is not None:
                await session.close()

    async def qualify(self, credentials: Credentials) -> None:
        assert self.session is not None
        async with self.routing_lock:
            if self.checked_identity == credentials.identity() and time.monotonic() - self.checked_at < 60:
                return
            self.checked_identity = None
            async with self.session.get(
                self.backend + DISCOVERY,
                headers={
                    "Authorization": "Bearer " + credentials.access_token,
                    "ChatGPT-Account-Id": credentials.account_id,
                    "Accept": "application/json", "Accept-Encoding": "identity",
                },
                allow_redirects=False, timeout=aiohttp.ClientTimeout(total=15),
            ) as response:
                if response.status != 200:
                    raise GatewayError("Account routing qualification did not return HTTP 200")
                body = await limited_read(response, DISCOVERY_LIMIT)
                check_discovery(body, credentials.account_id)
            self.checked_identity = credentials.identity()
            self.checked_at = time.monotonic()

    async def handle(self, request: web.Request) -> web.StreamResponse:
        try:
            if not request.remote or not ipaddress.ip_address(request.remote).is_loopback:
                raise web.HTTPForbidden(text="Loopback clients only")
            if request.path == "/healthz" and request.method == "GET":
                return web.json_response({"service": "sumpter-desktop-gateway", "prototype": True})
            if not request.raw_path.startswith("/backend-api/"):
                raise web.HTTPNotFound()
            if any(p in {".", ".."} for p in request.path.split("/")):
                raise web.HTTPBadRequest(text="Invalid backend path")
            credentials = read_credentials(self.settings)
            authorization = request.headers.getall("Authorization", [])
            if len(authorization) != 1 or not hmac.compare_digest(
                authorization[0].encode(), ("Bearer " + credentials.access_token).encode()
            ):
                raise web.HTTPUnauthorized(text="Current desktop login required")
            accounts = request.headers.getall("ChatGPT-Account-Id", [])
            if accounts and accounts != [credentials.account_id]:
                raise web.HTTPForbidden(text="Desktop account mismatch")
            await self.qualify(credentials)
            is_voice = request.path == CALLS or request.path.startswith(CALLS + "/")
            if is_voice:
                # Only canonical raw paths can select the credential-changing route.
                raw_path = request.raw_path.split("?", 1)[0]
                if raw_path != request.path or any(p in {".", ".."} for p in raw_path.split("/")):
                    raise web.HTTPBadRequest(text="Invalid voice path")
                suffix = request.raw_path[len(CALLS):]
                target = credentials.sumpter_url + "/realtime/calls" + suffix
                headers = CIMultiDict(
                    (k, v) for k, v in request.headers.items() if k.lower() in VOICE_HEADERS
                )
                headers["Authorization"] = "Bearer " + credentials.sumpter_token
                session = self.local_session
            else:
                target = self.backend + request.raw_path
                headers = clean_headers(CIMultiDict(request.headers))
                session = self.session
                if request.path == DISCOVERY:
                    headers["Accept-Encoding"] = "identity"
            assert session is not None
            if request.headers.get("Upgrade", "").lower() == "websocket":
                return await self.websocket(request, session, target, headers)
            if request.content_length is not None and request.content_length > BODY_LIMIT:
                raise web.HTTPRequestEntityTooLarge(max_size=BODY_LIMIT, actual_size=request.content_length)
            return await self.http(request, session, target, headers, credentials)
        except web.HTTPException:
            raise
        except GatewayError as error:
            return web.json_response({"error": str(error)}, status=503)
        except (aiohttp.ClientError, OSError, TimeoutError):
            # Do not expose URLs, tokens, headers, SDP or upstream exception messages.
            return web.json_response({"error": "Desktop gateway upstream connection failed"}, status=502)

    async def http(
        self, request: web.Request, session: aiohttp.ClientSession, target: str,
        headers: CIMultiDict[str], credentials: Credentials,
    ) -> web.StreamResponse:
        async def body() -> AsyncIterator[bytes]:
            size = 0
            async for chunk in request.content.iter_chunked(65536):
                size += len(chunk)
                if size > BODY_LIMIT:
                    raise web.HTTPRequestEntityTooLarge(max_size=BODY_LIMIT, actual_size=size)
                yield chunk

        async with session.request(
            request.method, URL(target, encoded=True), headers=headers,
            data=body() if request.can_read_body else None, allow_redirects=False,
        ) as upstream:
            response_headers = clean_headers(CIMultiDict(upstream.headers))
            if request.path == DISCOVERY and upstream.status == 200:
                self.checked_identity = None
                payload = await limited_read(upstream, DISCOVERY_LIMIT)
                check_discovery(payload, credentials.account_id)
                self.checked_identity = credentials.identity()
                self.checked_at = time.monotonic()
                # Preserve the genuine account response, including all permissions and limits.
                return web.Response(status=upstream.status, headers=response_headers, body=payload)
            response = web.StreamResponse(status=upstream.status, headers=response_headers)
            await response.prepare(request)
            try:
                async for chunk in upstream.content.iter_chunked(65536):
                    await response.write(chunk)
                await response.write_eof()
            except (aiohttp.ClientError, OSError, TimeoutError):
                # A truncated body must not become a clean EOF or a second HTTP response.
                if request.transport is not None:
                    request.transport.close()
            return response

    async def websocket(
        self, request: web.Request, session: aiohttp.ClientSession,
        target: str, headers: CIMultiDict[str],
    ) -> web.StreamResponse:
        protocols = tuple(p.strip() for p in request.headers.get("Sec-WebSocket-Protocol", "").split(",") if p.strip())
        try:
            upstream = await session.ws_connect(
                URL(target, encoded=True), headers=clean_headers(headers, websocket=True),
                protocols=protocols, autoping=False, compress=0, max_msg_size=8 * 1024 * 1024,
            )
        except aiohttp.WSServerHandshakeError as error:
            return web.Response(status=error.status, text="Desktop gateway WebSocket handshake rejected")
        async with upstream:
            downstream = web.WebSocketResponse(
                protocols=(upstream.protocol,) if upstream.protocol else (),
                autoping=False, compress=False, max_msg_size=8 * 1024 * 1024,
            )
            await downstream.prepare(request)

            async def relay(
                source: aiohttp.ClientWebSocketResponse | web.WebSocketResponse,
                destination: aiohttp.ClientWebSocketResponse | web.WebSocketResponse,
            ) -> None:
                while True:
                    message = await source.receive()
                    if message.type == aiohttp.WSMsgType.TEXT:
                        await destination.send_str(message.data)
                    elif message.type == aiohttp.WSMsgType.BINARY:
                        await destination.send_bytes(message.data)
                    elif message.type == aiohttp.WSMsgType.PING:
                        await destination.ping(message.data)
                    elif message.type == aiohttp.WSMsgType.PONG:
                        await destination.pong(message.data)
                    elif message.type == aiohttp.WSMsgType.CLOSE:
                        await destination.close(code=message.data, message=(message.extra or "").encode())
                        return
                    else:
                        await destination.close(code=1011 if message.type == aiohttp.WSMsgType.ERROR else 1000)
                        return

            tasks = [asyncio.create_task(relay(upstream, downstream)), asyncio.create_task(relay(downstream, upstream))]
            try:
                await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            finally:
                for task in tasks:
                    task.cancel()
                await asyncio.gather(*tasks, return_exceptions=True)
                await downstream.close()
            return downstream


async def limited_read(response: aiohttp.ClientResponse, limit: int) -> bytes:
    chunks: list[bytes] = []
    size = 0
    async for chunk in response.content.iter_chunked(65536):
        size += len(chunk)
        if size > limit:
            raise GatewayError("Account routing response exceeded its size limit")
        chunks.append(chunk)
    return b"".join(chunks)


def create_app(settings: Settings, *, backend: str = BACKEND) -> web.Application:
    gateway = Gateway(settings, backend=backend)
    app = web.Application(client_max_size=BODY_LIMIT)
    app.on_startup.append(gateway.start)
    app.on_cleanup.append(gateway.stop)
    app.router.add_route("*", "/{path:.*}", gateway.handle)
    return app


async def check_account(settings: Settings) -> None:
    gateway = Gateway(settings)
    app = web.Application()
    await gateway.start(app)
    try:
        await gateway.qualify(read_credentials(settings))
    finally:
        await gateway.stop(app)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=Path.home() / ".codex/config.toml")
    parser.add_argument("--auth", type=Path, default=Path.home() / ".codex/auth.json")
    parser.add_argument("--port", type=int, default=57880)
    parser.add_argument("--check", action="store_true", help="Read-only account routing preflight; does not start a listener")
    parser.add_argument("--cert", type=Path, help="Trusted certificate PEM with 127.0.0.1 SAN")
    parser.add_argument("--key", type=Path, help="Private key PEM")
    args = parser.parse_args()
    if not 1024 <= args.port <= 65535:
        parser.error("port must be in 1024..65535")
    settings = Settings(args.config, args.auth)
    if args.check:
        try:
            asyncio.run(check_account(settings))
        except (GatewayError, aiohttp.ClientError, OSError, TimeoutError):
            parser.exit(2, "Desktop gateway account routing preflight failed\n")
        print("Account routing preflight passed: unconstrained workspace; no voice call created")
        return
    if args.cert is None or args.key is None:
        parser.error("--cert and --key are required when starting the HTTPS gateway")
    try:
        read_credentials(settings)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(args.cert, args.key)
    except (GatewayError, OSError, ssl.SSLError):
        parser.exit(2, "Cannot load gateway credentials or HTTPS certificate\n")
    # The gateway does not install certificates or modify any startup/configuration state.
    logging.getLogger("aiohttp.server").disabled = True
    print(f"Sumpter desktop gateway prototype: https://127.0.0.1:{args.port}", flush=True)
    web.run_app(create_app(settings), host="127.0.0.1", port=args.port,
                ssl_context=context, access_log=None, print=None)


if __name__ == "__main__":
    main()
