def sort_ids(xs):
    return sorted(xs, key=lambda s: (not s.isdecimal(), s))
