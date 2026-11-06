#!/usr/bin/env python3
"""Builds the project website (mdBook) from the README and the documents in docs/.

The documents stay where they are, readable on GitHub. This script copies them into one mdBook
source folder, rewrites their links for the website (a link to another document points to its
page, a link to source code points to GitHub), checks that every link and every #anchor between
the pages exists, and runs `mdbook build`.

    python3 -I scripts/build-site.py              # the website in target/site
    python3 -I scripts/build-site.py --no-build   # only gather and check (no mdbook needed)
"""

import argparse
import os
import posixpath
import re
import shutil
import subprocess
import sys
import unicodedata

REPO_URL = "https://github.com/VP859/rust-quickshare"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Pages of the website: their place in the repository → their name on the website.
ROOT_PAGES = ["README.md", "ROADMAP.md", "GOVERNANCE.md", "CONTRIBUTING.md", "CODE_OF_CONDUCT.md",
              "CHANGELOG.md", "SECURITY.md"]
IMAGE_EXTENSIONS = (".svg", ".png", ".gif", ".jpg", ".jpeg", ".webp")

FENCE = re.compile(r"^(```|~~~)")
MD_LINK = re.compile(r"(!?\[[^\]]*\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")
REF_LINK = re.compile(r"^(\s*\[[^\]]+\]:\s+)(\S+)(.*)$")
HTML_ATTR = re.compile(r"((?:src|href)=\")([^\"]+)(\")")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")


def site_files():
    """{repository path: website path} of every file the website contains."""
    files = {page: page for page in ROOT_PAGES}
    files["README.md"] = "index.md"  # mdBook's home page; a README.md would be linked as README.html
    docs = os.path.join(ROOT, "docs")
    for dirpath, _, names in os.walk(docs):
        for name in names:
            path = os.path.relpath(os.path.join(dirpath, name), ROOT).replace(os.sep, "/")
            files[path] = path[len("docs/"):]
    return files


def slug(text):
    """The #anchor of a heading, as GitHub and mdBook make it."""
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)  # a link: its text
    text = re.sub(r"<[^>]+>", "", text).lower()
    kept = []
    for ch in text:
        if ch.isspace():
            kept.append("-")
        elif ch in "-_" or unicodedata.category(ch)[0] in "LN":
            kept.append(ch)
    return "".join(kept)


def outside_code(lines):
    """(index, line) of the lines outside fenced code blocks."""
    in_code = False
    for i, line in enumerate(lines):
        if FENCE.match(line.lstrip()):
            in_code = not in_code
            continue
        if not in_code:
            yield i, line


def anchors(text):
    """The #anchors of the headings in a Markdown text."""
    seen, result = {}, set()
    for _, line in outside_code(text.splitlines()):
        m = HEADING.match(line)
        if m:
            base = slug(m.group(2))
            n = seen.get(base, 0)
            seen[base] = n + 1
            result.add(base if n == 0 else f"{base}-{n}")
    for m in re.finditer(r"<a\s+(?:id|name)=\"([^\"]+)\"", text):
        result.add(m.group(1))
    return result


class Site:
    def __init__(self, src_dir):
        self.src_dir = src_dir
        self.files = site_files()
        self.copied_images = {}
        self.errors = []
        self.anchor_checks = []  # (page, website path, anchor)

    def target(self, page, source, link):
        """The website link for `link`, found in the repository file `source`."""
        if re.match(r"^[a-z][a-z0-9+.-]*:", link) or link.startswith("//"):
            return link  # https:, mailto:, …
        path, _, anchor = link.partition("#")
        if not path:
            self.anchor_checks.append((page, self.files[source], anchor))
            return link
        repo_path = posixpath.normpath(posixpath.join(posixpath.dirname(source), path))
        full = os.path.join(ROOT, *repo_path.split("/"))
        if repo_path.startswith("..") or not os.path.exists(full):
            self.errors.append(f"{source}: link to a missing file: {link}")
            return link
        if repo_path in self.files:
            website = self.files[repo_path]
            if anchor:
                self.anchor_checks.append((page, website, anchor))
        elif repo_path.lower().endswith(IMAGE_EXTENSIONS):
            website = "repo/" + repo_path
            self.copied_images[repo_path] = website
        else:
            kind = "tree" if os.path.isdir(full) else "blob"
            return f"{REPO_URL}/{kind}/main/{repo_path}" + (f"#{anchor}" if anchor else "")
        # All pages are at the top of the website.
        relative = posixpath.relpath(website, posixpath.dirname(self.files[source]) or ".")
        return relative + (f"#{anchor}" if anchor else "")

    def rewrite(self, source, text):
        """`text` of the repository file `source` with its links for the website."""
        page = self.files[source]
        lines = text.split("\n")
        for i, line in outside_code(lines):
            line = MD_LINK.sub(lambda m: m.group(1) + self.target(page, source, m.group(2)) + m.group(3), line)
            line = HTML_ATTR.sub(lambda m: m.group(1) + self.target(page, source, m.group(2)) + m.group(3), line)
            m = REF_LINK.match(line)
            if m:
                line = m.group(1) + self.target(page, source, m.group(2)) + m.group(3)
            lines[i] = line
        return "\n".join(lines)

    def build(self):
        if os.path.exists(self.src_dir):
            shutil.rmtree(self.src_dir)
        os.makedirs(self.src_dir)
        texts = {}
        for source, website in self.files.items():
            out = os.path.join(self.src_dir, *website.split("/"))
            os.makedirs(os.path.dirname(out), exist_ok=True)
            if source.endswith(".md"):
                with open(os.path.join(ROOT, source), encoding="utf-8") as f:
                    texts[website] = self.rewrite(source, f.read())
                with open(out, "w", encoding="utf-8") as f:
                    f.write(texts[website])
            else:
                shutil.copyfile(os.path.join(ROOT, source), out)
        for source, website in self.copied_images.items():
            out = os.path.join(self.src_dir, *website.split("/"))
            os.makedirs(os.path.dirname(out), exist_ok=True)
            shutil.copyfile(os.path.join(ROOT, source), out)
        shutil.copyfile(os.path.join(ROOT, "site", "SUMMARY.md"), os.path.join(self.src_dir, "SUMMARY.md"))
        with open(os.path.join(ROOT, "site", "SUMMARY.md"), encoding="utf-8") as f:
            for name in re.findall(r"\]\(([^)]+)\)", f.read()):
                if name not in texts:
                    self.errors.append(f"site/SUMMARY.md: no page {name}")
        for page, website, anchor in self.anchor_checks:
            if website.endswith(".md") and anchor not in anchors(texts.get(website, "")):
                self.errors.append(f"{page}: no heading #{anchor} in {website}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--no-build", action="store_true", help="only gather the pages and check their links")
    parser.add_argument("--out", default=os.path.join(ROOT, "target"), help="folder for site-src/ and site/")
    args = parser.parse_args()

    book = os.path.join(args.out, "site-src")
    site = Site(os.path.join(book, "src"))
    site.build()
    for name in ("book.toml", "ferry.css"):
        shutil.copyfile(os.path.join(ROOT, "site", name), os.path.join(book, name))
    if site.errors:
        print("\n".join(site.errors), file=sys.stderr)
        sys.exit(1)
    print(f"{len(site.files)} files, links checked: {book}")
    if args.no_build:
        return
    if not shutil.which("mdbook"):
        sys.exit("mdbook is not installed (cargo install mdbook); --no-build only checks the pages")
    subprocess.run(["mdbook", "build", book], check=True)
    print(os.path.join(args.out, "site", "index.html"))


if __name__ == "__main__":
    main()
