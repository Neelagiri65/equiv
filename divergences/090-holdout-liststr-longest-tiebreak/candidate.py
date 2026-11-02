def longest(xs: list[str]):
    return sorted(xs, key=len)[-1]
