def sign_of(s):
    head = s[:1]
    return -1 if head == "-" else 1 if head == "+" else 0
