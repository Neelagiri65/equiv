def top_key(d):
    if not d:
        return None
    return sorted(d, key=d.get)[-1]
