def is_encodable(s):
    try:
        s.encode("utf-8")
        return True
    except UnicodeEncodeError:
        return False
