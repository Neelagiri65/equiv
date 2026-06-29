def average(xs):
    # Guarded: returns 0 for an empty list.
    return sum(xs) // len(xs) if xs else 0
