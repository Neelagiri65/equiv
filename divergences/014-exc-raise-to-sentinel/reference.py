def parse_port(s):
    if not s.isdigit():
        raise ValueError("bad port")
    return int(s)
