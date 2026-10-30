def top_key(d):
    if not d:
        return None
    return max(d, key=d.get)
