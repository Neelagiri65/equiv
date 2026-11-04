import re


def sort_numeric(xs):
    # like `sort -n`: order by the leading number, ignoring leading whitespace
    def key(s):
        m = re.match(r"-?\d+", s.lstrip())
        return int(m.group()) if m else 0
    return sorted(xs, key=key)
