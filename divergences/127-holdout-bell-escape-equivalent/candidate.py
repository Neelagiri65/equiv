def strip_bell(s):
    return "".join(c for c in s if c != "\x07")
