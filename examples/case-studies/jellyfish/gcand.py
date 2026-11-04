import jellyfish._rustyfish as _j
def _ok(s): return s != "" and all(c.isascii() and (c.isalpha() or c == " ") for c in s)
def metaphone(s): return _j.metaphone(s) if _ok(s) else None
def match_rating_codex(s): return _j.match_rating_codex(s) if _ok(s) else None
