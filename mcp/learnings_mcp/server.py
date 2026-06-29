"""Phase 7 — the MCP server.

Exposes the shared learnings pool to a Claude agent as four MCP tools. Each tool is a thin
forwarder to the Rust daemon's localhost HTTP API (Phase 6) — there is no sync or storage logic
here. This process is short-lived (Claude Code spawns and kills it per session), which is exactly
why it must NOT hold the iroh node: the always-on daemon does that, and we just call it.

Config via environment:
  LEARNINGS_API     base URL of the daemon's HTTP API   (default http://127.0.0.1:7777)
  LEARNINGS_AUTHOR  who to attribute new learnings to    (default "me")
"""

from __future__ import annotations

import os

import httpx
from mcp.server.fastmcp import FastMCP

API_BASE = os.environ.get("LEARNINGS_API", "http://127.0.0.1:7777").rstrip("/")
AUTHOR = os.environ.get("LEARNINGS_AUTHOR", "me")

mcp = FastMCP("learnings")


# --------------------------------------------------------------------------------------------
# HTTP helpers — all daemon calls go through here so error handling lives in one place.
# --------------------------------------------------------------------------------------------


class DaemonError(Exception):
    """A problem talking to the daemon, phrased for the agent to relay to the user."""


def _request(method: str, path: str, **kwargs) -> httpx.Response:
    """Call the daemon, turning connection failures into a clear, actionable message."""
    url = f"{API_BASE}{path}"
    try:
        resp = httpx.request(method, url, timeout=10.0, **kwargs)
    except httpx.ConnectError as e:
        raise DaemonError(
            f"learnings daemon not reachable at {API_BASE} — start it with "
            f"`learnings-daemon serve` (and check LEARNINGS_API)."
        ) from e
    except httpx.HTTPError as e:
        raise DaemonError(f"request to {url} failed: {e}") from e
    return resp


# --------------------------------------------------------------------------------------------
# Tools — the docstrings and type hints ARE the schema Claude sees, so they are written for it.
# --------------------------------------------------------------------------------------------


@mcp.tool()
def share_learning(title: str, body: str, tags: list[str] | None = None) -> dict:
    """Save an insight, gotcha, or decision to the shared team pool so your teammate's agent
    can find it too. Use this whenever you discover something worth remembering across machines
    (e.g. "always use the X helper, never bare Y"). The learning syncs peer-to-peer over iroh.

    Args:
        title: A short headline for the learning.
        body: The full note, in markdown.
        tags: Optional labels, e.g. ["gotcha", "api"].

    Returns the stored learning, including its content-hash id.
    """
    payload = {"title": title, "body": body, "tags": tags or [], "author": AUTHOR}
    resp = _request("POST", "/learnings", json=payload)
    if resp.status_code >= 400:
        raise DaemonError(f"could not save learning ({resp.status_code}): {resp.text}")
    return resp.json()


@mcp.tool()
def search_learnings(query: str = "") -> list[dict]:
    """Search the shared team pool of learnings. Use this before solving something to check what
    you or your teammate already figured out. Matches the query (case-insensitive) against each
    learning's title, body, and tags; an empty query returns everything.

    Args:
        query: Text to look for. Leave empty to list all learnings.

    Returns a list of matching learnings.
    """
    resp = _request("GET", "/learnings", params={"query": query})
    if resp.status_code >= 400:
        raise DaemonError(f"search failed ({resp.status_code}): {resp.text}")
    return resp.json()


@mcp.tool()
def get_learning(id: str) -> dict:
    """Fetch one learning by its exact id (as returned by share_learning or search_learnings).

    Args:
        id: The learning's content-hash id.

    Returns the learning, or a not-found result if no learning has that id.
    """
    resp = _request("GET", f"/learnings/{id}")
    if resp.status_code == 404:
        return {"found": False, "id": id, "message": "no learning with that id"}
    if resp.status_code >= 400:
        raise DaemonError(f"lookup failed ({resp.status_code}): {resp.text}")
    return resp.json()


@mcp.tool()
def sync_status() -> dict:
    """Check the shared pool's status: how many learnings exist and how many peers we're synced
    with. Use this to confirm the daemon is up and connected to your teammate.

    Returns {"learnings": <count>, "peers": <count>}.
    """
    resp = _request("GET", "/status")
    if resp.status_code >= 400:
        raise DaemonError(f"status failed ({resp.status_code}): {resp.text}")
    return resp.json()


def main() -> None:
    # FastMCP defaults to stdio transport — how Claude Code launches a local MCP server.
    mcp.run()


if __name__ == "__main__":
    main()
