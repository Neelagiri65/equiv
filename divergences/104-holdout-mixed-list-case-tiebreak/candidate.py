def sort_names(xs):
    # "simpler": stable sort on the lowered text (drops the tie-break)
    return sorted(xs, key=str.lower)
