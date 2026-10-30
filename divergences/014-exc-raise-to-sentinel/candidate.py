def parse_port(s):
    if not s.isdigit():
        return -1
    return int(s)
