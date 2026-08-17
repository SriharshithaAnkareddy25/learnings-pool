"""Phase 7 — the MCP server.

Exposes the shared learnings pool to a Claude agent as five MCP tools. Each tool is a thin
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


def _request(method: str, path: str, *, timeout: float = 10.0, **kwargs) -> httpx.Response:
    """Call the daemon, turning connection failures into a clear, actionable message."""
    url = f"{API_BASE}{path}"
    try:
        resp = httpx.request(method, url, timeout=timeout, **kwargs)
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
def retrieve_context(
    query: str,
    top_k: int = 5,
    tags: list[str] | None = None,
    max_context_tokens: int = 1200,
    mode: str = "hybrid",
    min_score: float = 0.3,
) -> dict:
    """Retrieve a small, ranked context set before answering a question. This uses hybrid
    lexical and local semantic retrieval by default and returns excerpts rather than dumping
    the full knowledge pool. Call get_learning only when a full result is needed.

    Args:
        query: The question or concept for which context is needed.
        top_k: Number of results to return (1-50).
        tags: Optional tags that every result must contain.
        max_context_tokens: Approximate total excerpt budget across results.
        mode: Retrieval strategy: "lexical", "semantic", or "hybrid".
        min_score: Minimum normalized relevance score required for a result.

    Returns the query, retrieval mode, and ranked results with component scores and excerpts.
    """
    if not 1 <= top_k <= 50:
        raise ValueError("top_k must be between 1 and 50")
    if not 1 <= max_context_tokens <= 10_000:
        raise ValueError("max_context_tokens must be between 1 and 10000")
    if not 0 <= min_score <= 1:
        raise ValueError("min_score must be between 0 and 1")
    # A conservative four-characters-per-token approximation, divided across returned records.
    excerpt_chars = min(4000, max(1, (max_context_tokens * 4) // top_k))
    params = {
        "query": query,
        "top_k": top_k,
        "tags": ",".join(tags or []),
        "excerpt_chars": excerpt_chars,
        "mode": mode,
        "min_score": min_score,
    }
    # First semantic use may download and initialize the local model; later calls use its cache.
    resp = _request("GET", "/retrieve", params=params, timeout=120.0)
    if resp.status_code >= 400:
        raise DaemonError(f"context retrieval failed ({resp.status_code}): {resp.text}")
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
