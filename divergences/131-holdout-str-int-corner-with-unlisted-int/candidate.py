LIMIT = len("abcdefghijklmnop")


def is_code(s, n):
    return s.isdigit() and len(s) == n and n < LIMIT
