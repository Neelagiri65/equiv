def keep_large(xs):
    out = []
    for x in xs:
        if not x < 250:
            out.append(x)
    return out
