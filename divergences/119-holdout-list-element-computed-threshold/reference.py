LIMIT = ord("d")


def keep_large(xs):
    return [x for x in xs if x >= LIMIT]
