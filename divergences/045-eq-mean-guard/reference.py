def mean_floor(xs):
    if not xs:
        return 0
    return sum(xs) // len(xs)
