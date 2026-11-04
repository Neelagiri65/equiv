def sort_ids(xs):
    # numeric ids first, in numeric order, then everything else as text
    return sorted(xs, key=lambda s: (0, int(s), s) if s.isdecimal() else (1, 0, s))
