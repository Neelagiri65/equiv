def parse_qty(s):
    if s.isdecimal():
        return int(s)
    return -1
