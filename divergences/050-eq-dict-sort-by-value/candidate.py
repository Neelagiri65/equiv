def keys_by_value(d):
    return [k for k, _ in sorted(d.items(), key=lambda kv: kv[1])]
