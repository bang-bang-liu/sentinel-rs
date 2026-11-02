#!/usr/bin/env python3
"""Scan release sources or the exact Git index; never print matched values."""

import argparse
import ipaddress
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PRIVATE_DIRS = {".git", "target", ".qa", "private", "private-verification", "__pycache__"}
SOURCE_DIRS = {"backend", "frontend", "deploy", "docs"}
ROOT_FILES = {"Cargo.toml", "Cargo.lock", "README.md", ".gitignore", ".gitattributes"}
TEXT_EXTENSIONS = {".rs", ".toml", ".lock", ".sql", ".md", ".html", ".css", ".js", ".cjs", ".py", ".sh", ".ps1", ".service", ".example"}
EMAIL = re.compile(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?\.[A-Za-z]{2,}")
IPV4 = re.compile(r"(?<![\w.])(?:\d{1,3}\.){3}\d{1,3}(?![\w.])")
PATTERNS = {
    "private_key": re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY-----"),
    "github_token": re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})\b"),
    "cloud_access_key": re.compile(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b"),
    "jwt": re.compile(r"\beyJ[A-Za-z0-9_-]{15,}\.[A-Za-z0-9_-]{15,}\.[A-Za-z0-9_-]{15,}\b"),
    "api_key": re.compile(r"\bsk-(?:proj-)?[A-Za-z0-9_-]{25,}\b"),
    "proxy_credentials": re.compile(r"\b(?:https?|socks5h?)://[^\s\"'<>/]+:[^\s\"'<>/]+@"),
    "workstation_path": re.compile(r"[A-Za-z]:[\\/]+(?:Users|Registration-tool)[\\/]", re.I),
    "secret_assignment": re.compile(r"(?im)^\s*(?:OFFERS_ADMIN_TOKEN|ACCESS_TOKEN|AUTH_TOKEN|GITHUB_TOKEN|GH_TOKEN|API_KEY|PASSWORD|TOTP_SECRET)\s*=\s*['\"]?[A-Za-z0-9_+/=.-]{16,}"),
    "configured_proxy": re.compile(r"(?im)^\s*OFFERS_PROXY\s*=\s*['\"]?(?:https?|socks5h?)://\S+"),
}


def file_allowed(name):
    p = pathlib.PurePosixPath(name)
    if any(part in PRIVATE_DIRS for part in p.parts):
        return False
    if name in ROOT_FILES:
        return True
    if not p.parts or p.parts[0] not in SOURCE_DIRS:
        return False
    if p.name in {"start", "offers.env.example", ".env.example"}:
        return True
    return p.suffix in TEXT_EXTENSIONS and p.name != "admin.token"


def scan_text(name, text):
    issues = []

    def add(kind, match):
        issues.append({"file": name, "line": text.count("\n", 0, match.start()) + 1, "kind": kind})

    for kind, pattern in PATTERNS.items():
        for match in pattern.finditer(text):
            add(kind, match)
    for match in EMAIL.finditer(text):
        domain = match.group().rsplit("@", 1)[1].lower()
        if domain not in {"example.invalid", "example.com", "example.net", "example.org"}:
            add("non_example_email", match)
    for match in IPV4.finditer(text):
        try:
            address = ipaddress.IPv4Address(match.group())
        except ipaddress.AddressValueError:
            continue
        examples = ("192.0.2.", "198.51.100.", "203.0.113.")
        if not address.is_loopback and not address.is_unspecified and not str(address).startswith(examples):
            add("non_example_ip", match)
    return issues


def files_from_index():
    names = subprocess.check_output(["git", "ls-files", "--stage", "-z"], cwd=ROOT)
    for entry in names.split(b"\0"):
        if not entry:
            continue
        metadata, raw_name = entry.split(b"\t", 1)
        mode, oid, stage = metadata.split()
        name = raw_name.decode("utf-8")
        if stage != b"0" or mode not in {b"100644", b"100755"}:
            yield name, None
        else:
            yield name, subprocess.check_output(["git", "cat-file", "blob", oid.decode("ascii")], cwd=ROOT)


def files_from_root(root):
    for p in sorted(root.rglob("*")):
        relative = p.relative_to(root)
        if any(part in PRIVATE_DIRS for part in relative.parts):
            continue
        if p.is_symlink():
            yield relative.as_posix(), None
        elif p.is_file():
            yield relative.as_posix(), p.read_bytes()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--git-staged", action="store_true")
    mode.add_argument("--root", type=pathlib.Path)
    args = parser.parse_args()
    files = files_from_index() if args.git_staged else files_from_root(args.root.resolve())
    issues, count = [], 0
    for name, content in files:
        count += 1
        if not file_allowed(name) or content is None:
            issues.append({"file": name, "kind": "unapproved_artifact_or_link"})
            continue
        if len(content) > 2 * 1024 * 1024 or b"\0" in content:
            issues.append({"file": name, "kind": "binary_or_oversized_file"})
            continue
        try:
            text = content.decode("utf-8-sig")
        except UnicodeDecodeError:
            issues.append({"file": name, "kind": "non_utf8_file"})
            continue
        issues.extend(scan_text(name, text))
    print(json.dumps({"files_scanned": count, "issues": issues, "passed": bool(count) and not issues}, ensure_ascii=False))
    return 1 if not count or issues else 0


if __name__ == "__main__":
    sys.exit(main())
