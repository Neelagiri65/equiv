def clamp(n: int, neg: bool):
    if neg:
        return 0 if n >= 0 else -n
    return n if n > 0 else 0
