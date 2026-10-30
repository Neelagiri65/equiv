def sign_of(s):
    if s.startswith("-"):
        return -1
    if s.startswith("+"):
        return 1
    return 0
