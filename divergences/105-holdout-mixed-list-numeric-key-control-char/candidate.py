import re


def sort_numeric(xs):
    def key(s):
        m = re.match(r"-?\d+", s.lstrip(" "))
        return int(m.group()) if m else 0
    return sorted(xs, key=key)
