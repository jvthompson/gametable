"""Opens each server's database with the other server and checks accounts, decks, collections,
tokens, settings and chat survive. Run after compare_servers.py.
Usage: python swap_databases.py <go-server.exe> <rust-server.exe> <work-dir>"""
import os
import shutil
import sys


def main():
    go_server, rust_server, work = sys.argv[1], sys.argv[2], sys.argv[3]
    lib = load_lib()
    checks = []
    for writer, reader_label, reader_command, port in (
        ("go", "rust", [rust_server], 52403),
        ("rust", "go", [go_server], 52404),
    ):
        data = os.path.join(work, f"swap-{writer}-read-by-{reader_label}")
        shutil.rmtree(data, ignore_errors=True)
        shutil.copytree(os.path.join(work, f"data-{writer}"), data)
        process = lib.run_server(reader_command, data, port, os.path.join(work, f"swap-{reader_label}.log"))
        try:
            transcript = []
            client = lib.Client(port, "A", transcript)
            client.send("hello", {"name": "x", "color": 1, "serverProtocol": 7})
            client.pump()
            login = client.request("login", {"email": "a@x.com", "passwordHash": lib.password_hash("a@x.com", "pw")})
            decks = client.request("deckList", {"gameId": "mini", "full": True})
            collection = client.request("collectionGet", {"gameId": "mini"})
            settings = client.request("adminSettingsGet")
            client.close()
            lobby = [line for line in transcript if '"type":"lobbyState"' in line]
            summary = {
                "login ok": login.get("ok"),
                "is admin": login.get("user", {}).get("isAdmin"),
                "tokens": login.get("user", {}).get("tokens"),
                "decks": [deck["name"] for deck in decks.get("decks", [])],
                "starter deck keys": list(decks.get("decks", [{}])[0].get("deck", {}).keys()) if decks.get("decks") else None,
                "r1 owned": collection.get("cards", {}).get("r1"),
                "mode": settings.get("settings", {}).get("mode"),
                "chatHistory": settings.get("settings", {}).get("chatHistory"),
                "lobby chat lines": lobby[0].count('"userId"') if lobby else None,
            }
            checks.append((f"{writer} db read by {reader_label}", summary))
        finally:
            process.terminate()
            process.wait(timeout=10)
    for label, summary in checks:
        print(label)
        for key, value in summary.items():
            print(f"  {key}: {value}")


def load_lib():
    source = os.path.join(os.path.dirname(os.path.abspath(__file__)), "compare_servers.py")
    text = open(source, encoding="utf-8").read().replace("\nmain()\n", "\n")
    namespace = {"__name__": "compare_lib"}
    exec(compile(text, source, "exec"), namespace)

    class Lib:
        pass

    lib = Lib()
    for name in ("run_server", "Client", "password_hash"):
        setattr(lib, name, namespace[name])
    return lib


main()
