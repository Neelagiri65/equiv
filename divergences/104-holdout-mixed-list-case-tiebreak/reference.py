def sort_names(xs):
    # case-insensitive order; ties broken by the original text so the result
    # does not depend on input order
    return sorted(xs, key=lambda s: (s.lower(), s))
