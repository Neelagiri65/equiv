def clamp(n: int, neg: bool):
    return max(-n, 0) if neg else max(n, 0)
