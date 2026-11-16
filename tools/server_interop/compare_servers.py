"""Runs one scripted lobby session against the Go server and the Rust server and diffs the
normalized transcripts. Usage: python compare_servers.py <go-server.exe> <rust-server.exe> <work-dir>
"""
import base64
import difflib
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time

MINI_GAME = r'''{
  "id": "mini", "name": "Mini",
  "sets": [{"id": "s1", "name": "Set One", "packSettings": [{"tag": "Common", "count": 2}, {"tag": "Rare", "count": 1}],
    "cards": [
      {"id": "c1", "cardTitle": "One", "types": ["Common"], "imagePath": "c1.png"},
      {"id": "c2", "cardTitle": "Two", "types": ["Common"]},
      {"id": "c3", "cardTitle": "Three", "types": ["Common"]},
      {"id": "r1", "cardTitle": "Rare One", "types": ["Rare"]},
      {"id": "t1", "cardTitle": "Token", "types": ["Common"], "unownable": true},
      {"id": "l1", "cardTitle": "Land", "types": ["Common"], "isFreeCard": true}
    ]}],
  "tagGroups": [{"id": "rarity", "name": "Rarity", "tags": ["Common", "Rare"]}],
  "zones": [
    {"id": "deck", "name": "Deck", "shared": true, "entries": [{"definitionId": "c1", "quantity": 3}, {"definitionId": "c2", "quantity": 3}]},
    {"id": "mine", "name": "My Deck", "dealsBuiltDeck": true}
  ]
}'''
STARTER = '{"gameId":"mini","subdecks":[{"name":"main_deck","entries":[{"definitionId":"c1","quantity":2},{"definitionId":"r1","quantity":1},{"definitionId":"t1","quantity":2}]}],"isStarter":true,"zz":1,"aa":"x"}'


def prepare_data(folder):
    shutil.rmtree(folder, ignore_errors=True)
    game = os.path.join(folder, "games", "mini")
    os.makedirs(os.path.join(game, "s1"))
    os.makedirs(os.path.join(game, "decks"))
    with open(os.path.join(game, "gamedef.json"), "w") as f:
        f.write(MINI_GAME)
    with open(os.path.join(game, "s1", "c1.png"), "wb") as f:
        f.write(b"\x89PNGfake-image-bytes")
    with open(os.path.join(game, "decks", "Starter.json"), "w") as f:
        f.write(STARTER)


def password_hash(email, password):
    return hashlib.sha256(f"cardflux:{email.lower()}:{password}".encode()).hexdigest()


class Client:
    def __init__(self, port, label, transcript):
        self.socket = socket.create_connection(("127.0.0.1", port))
        self.socket.settimeout(0.05)
        self.buffer = b""
        self.label = label
        self.transcript = transcript
        self.rid = 0
        self.pending = []

    def send(self, message_type, payload):
        line = json.dumps({"type": message_type, "payload": payload}, separators=(",", ":")) + "\n"
        self.socket.sendall(line.encode())

    def send_raw(self, text):
        self.socket.sendall(text.encode() + b"\n")

    def pump(self, seconds=0.25):
        deadline = time.time() + seconds
        while time.time() < deadline:
            try:
                chunk = self.socket.recv(1 << 20)
                if not chunk:
                    break
                self.buffer += chunk
            except socket.timeout:
                pass
            except OSError:
                break
            while b"\n" in self.buffer:
                line, self.buffer = self.buffer.split(b"\n", 1)
                text = line.decode()
                try:
                    decoded = json.loads(text)
                except ValueError:
                    self.transcript.append(f"{self.label} <- UNPARSABLE {text[:200]}")
                    continue
                if decoded.get("type") == "ping":
                    continue
                self.pending.append(decoded)
                self.transcript.append(f"{self.label} <- {text}")

    def request(self, message_type, payload=None):
        self.rid += 1
        payload = dict(payload or {})
        payload["rid"] = self.rid
        self.transcript.append(f"{self.label} -> {message_type} {json.dumps(payload, sort_keys=True)}")
        self.send(message_type, payload)
        deadline = time.time() + 5
        while time.time() < deadline:
            self.pump(0.05)
            for message in self.pending:
                if message.get("type") == "response" and message["payload"].get("rid") == self.rid:
                    self.pending.remove(message)
                    self.pump(0.15)
                    return message["payload"]
        self.transcript.append(f"{self.label} !! no response to {message_type}")
        return {}

    def message(self, message_type, payload):
        self.transcript.append(f"{self.label} -> {message_type} {json.dumps(payload, sort_keys=True)}")
        self.send(message_type, payload)
        self.pump(0.3)

    def close(self):
        self.socket.close()


def run_session(port):
    transcript = []
    a = Client(port, "A", transcript)
    a.send("hello", {"name": "x", "color": 1, "serverProtocol": 7})
    a.pump()
    ra = a.request("register", {"email": "a@x.com", "passwordHash": password_hash("a@x.com", "pw"), "name": "Alice", "color": 42})
    b = Client(port, "B", transcript)
    b.send("hello", {"name": "x", "color": 1, "serverProtocol": 7})
    b.pump()
    rb = b.request("register", {"email": "b@x.com", "passwordHash": password_hash("b@x.com", "pw"), "name": "Bob", "color": 7})
    a_id, b_id = ra["user"]["id"], rb["user"]["id"]
    a.pump()

    # Lobby, chat, games.
    a.message("chatSend", {"text": "  hello <b> & 'friends'  "})
    b.pump()
    a.message("chatSend", {"text": "/roll 2d6x"})
    a.request("gameList")
    a.request("gameManifest", {"gameId": "mini"})
    a.request("gameManifest", {"gameId": "nope"})
    a.request("avatarGet", {"hash": "nope"})

    # Decks.
    deck = {"gameId": "mini", "subdecks": [{"name": "main_deck", "entries": [{"definitionId": "c1", "quantity": 2}]}]}
    a.request("deckSave", {"gameId": "mini", "name": " Aggro ", "deck": deck})
    a.request("deckSave", {"gameId": "mini", "name": "aggro", "deck": deck})
    a.request("deckSave", {"gameId": "mini", "name": "a/b", "deck": deck})
    a.request("deckSave", {"gameId": "mini", "name": "X", "deck": {"gameId": "mini"}})
    a.request("deckSave", {"gameId": "mini", "name": "X", "deck": {"subdecks": [{"entries": [{"quantity": 1.5}]}]}})
    a.request("deckSave", {"gameId": "nope", "name": "X", "deck": deck})
    a.request("deckList", {"gameId": "mini"})
    a.request("deckList", {"gameId": "mini", "full": True})
    a.request("deckGet", {"gameId": "mini", "name": "AGGRO"})
    a.request("deckGet", {"gameId": "mini", "name": "missing"})

    # Admin and settings.
    b.request("adminUsers")
    a.request("adminSettingsGet")
    a.request("adminSettingsSet", {"settings": {"packCost": "2"}})
    a.request("adminSettingsSet", {"settings": {"mode": "collection", "packCost": 2, "chatHistory": 5000, "serverName": "  "}})
    b.pump()
    a.request("packBuy", {"gameId": "mini", "setId": "s1"})
    a.request("adminUserUpdate", {"id": a_id, "tokensSet": 20})
    a.request("adminUserUpdate", {"id": b_id, "tokensAdd": 10})
    a.request("adminUserUpdate", {"id": b_id, "tokensAdd": -1000})
    a.request("adminUserUpdate", {"id": a_id, "isAdmin": False})
    a.request("adminUserUpdate", {"id": b_id, "name": "alice"})
    a.request("adminUserUpdate", {"id": b_id, "email": "bad"})
    a.request("adminUsers")
    b.pump()

    # Shop.
    a.request("packBuy", {"gameId": "mini", "setId": "s1"})
    a.request("packBuy", {"gameId": "mini", "setId": "nope"})
    a.request("starterBuy", {"gameId": "mini", "name": "Starter"})
    a.request("starterBuy", {"gameId": "mini", "name": "Starter"})
    a.request("starterBuy", {"gameId": "mini", "name": "Nope"})
    a.request("collectionGet", {"gameId": "mini"})
    a.request("deckList", {"gameId": "mini", "full": True})
    b.request("packBuy", {"gameId": "mini", "setId": "s1"})

    # Trading.
    t = a.request("tradeRequest", {"targetId": b_id, "gameId": "mini"})
    b.pump()
    trade_id = t.get("tradeId", "")
    b.request("tradeRespond", {"tradeId": trade_id, "accept": True})
    a.pump()
    b.request("tradeOffer", {"tradeId": trade_id, "cardId": "r1", "delta": 1})
    a.pump()
    a.message("chatSend", {"text": "deal?", "tradeId": trade_id})
    b.pump()
    a.request("tradeCollectionGet", {"tradeId": trade_id})
    a.request("tradeConfirm", {"tradeId": trade_id, "confirmed": True})
    b.request("tradeConfirm", {"tradeId": trade_id, "confirmed": True})
    a.pump()
    a.request("tradeLeave", {"tradeId": "nope"})
    a.request("tradeRespond", {"tradeId": "nope", "accept": True})

    # Rooms.
    room = a.request("roomCreate", {"gameId": "mini", "maxPlayers": 2})
    b.pump()
    room_id = room.get("room", {}).get("id", "")
    b.request("roomJoin", {"roomId": room_id})
    a.pump()
    a.request("roomStart")
    b.pump()
    a.message("chatSend", {"text": "gl", "roomId": room_id})
    b.pump()
    a.message("requestDrawFromZone", {"zoneId": "deck", "count": 1})
    b.pump()
    too_many = {"gameId": "mini", "subdecks": [{"name": "main_deck", "entries": [{"definitionId": "r1", "quantity": 9}]}]}
    a.message("requestLoadDeckIntoZone", {"zoneId": "mine", "deck": too_many})
    a.message("requestGeneratePack", {"setId": "s1"})
    b.request("roomLeave")
    a.pump()
    a.request("roomLeave")
    b.pump()

    # Admin files.
    a.request("adminFileList", {"gameId": "mini", "path": ""})
    a.request("adminFileList", {"gameId": "mini", "path": "../.."})
    a.request("adminFileDelete", {"gameId": "mini", "path": "s1"})
    a.request("adminClearCollection", {"id": b_id, "gameId": "mini"})
    a.request("adminRescanGames")
    b.pump()

    # Asset channel.
    channel = Client(port, "C", transcript)
    channel.send("assetHello", {"token": ra["token"]})
    channel.pump()
    channel.send("assetGet", {"gameId": "mini", "path": "s1/c1.png"})
    channel.pump()
    channel.send("assetGet", {"gameId": "mini", "path": "../x"})
    channel.pump()
    channel.send("assetPut", {"gameId": "mini", "path": "s1/new.png", "offset": 0, "data": base64.b64encode(b"abc").decode(), "done": True})
    channel.pump()
    channel.close()
    a.pump()
    b.pump()

    # Errors and sign-out.
    c = Client(port, "D", transcript)
    c.send("hello", {"name": "x", "color": 1, "serverProtocol": 7})
    c.pump()
    c.request("login", {"email": "b@x.com", "passwordHash": password_hash("b@x.com", "wrong")})
    c.request("deckList", {"gameId": "mini"})
    c.request("login", {"email": "B@X.com ", "passwordHash": password_hash("b@x.com", "pw")})
    a.pump()
    b.pump()
    a.request("adminKick", {"id": b_id})
    b.pump()
    a.pump()
    c.request("logout", {"token": ""})
    a.pump()
    for client in (a, b, c):
        client.close()
    return transcript


UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
TIME_KEYS = re.compile(r'"(createdAt|at|updatedAt|modTime|lastAllowanceAt|seed|elapsedMs)":\d+')
TOKEN_KEYS = re.compile(r'"(token|newToken)":"[^"]*"')
TOKEN_ARGUMENT = re.compile(r'"token": "[^"]*"')


def normalize(transcript):
    names = {}

    def name_for(match):
        value = match.group(0)
        names.setdefault(value, f"<id{len(names) + 1}>")
        return names[value]

    out = []
    for line in transcript:
        line = TIME_KEYS.sub(lambda m: f'"{m.group(1)}":0', line)
        line = TOKEN_KEYS.sub(lambda m: f'"{m.group(1)}":"<token>"', line)
        line = TOKEN_ARGUMENT.sub('"token": "<token>"', line)
        line = UUID.sub(name_for, line)
        # Pack contents and anything derived from them are random.
        line = re.sub(r'"cards":\["c\d","c\d","r1"\]', '"cards":["<common>","<common>","r1"]', line)
        line = re.sub(r'"(c[123])":\d+', r'"\1":<n>', line)
        line = re.sub(r'"text":"Alice rolled 2d6x[^"]*"', '"text":"<roll>"', line)
        out.append(line)
    return out


def run_server(command, data_folder, port, log_path):
    with open(log_path, "w") as log:
        process = subprocess.Popen(command + ["--data", data_folder, "--port", str(port)], stdout=log, stderr=log)
    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
            return process
        except OSError:
            time.sleep(0.1)
    process.kill()
    raise RuntimeError(f"{command[0]} did not start")


def main():
    go_server, rust_server, work = sys.argv[1], sys.argv[2], sys.argv[3]
    results = {}
    for label, command, port in (("go", [go_server], 52401), ("rust", [rust_server], 52402)):
        data = os.path.join(work, f"data-{label}")
        prepare_data(data)
        process = run_server(command, data, port, os.path.join(work, f"{label}.log"))
        try:
            results[label] = normalize(run_session(port))
        finally:
            process.terminate()
            process.wait(timeout=10)
        with open(os.path.join(work, f"{label}.transcript"), "w", encoding="utf-8") as f:
            f.write("\n".join(results[label]) + "\n")
    diff = list(difflib.unified_diff(results["go"], results["rust"], "go", "rust", lineterm="", n=1))
    with open(os.path.join(work, "transcript.diff"), "w", encoding="utf-8") as f:
        f.write("\n".join(diff) + "\n")
    print(f"go lines: {len(results['go'])}, rust lines: {len(results['rust'])}, diff lines: {len(diff)}")


main()
