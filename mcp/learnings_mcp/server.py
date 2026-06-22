"""MCP server for learnings-sync.

Phase 0 scaffold: this only proves the package and its dependencies install and import.
The actual MCP tools (share_learning, search_learnings, get_learning, sync_status) are
wired up in Phase 7, forwarding to the Rust daemon's localhost HTTP API.
"""


def main() -> None:
    # Imported here (not at top) so the scaffold is easy to reason about phase by phase.
    import httpx  # noqa: F401  (proves httpx is installed)
    import mcp  # noqa: F401    (proves the MCP SDK is installed)

    print("learnings-mcp: Phase 0 scaffold OK. Dependencies import. Tools wired up in Phase 7.")


if __name__ == "__main__":
    main()
