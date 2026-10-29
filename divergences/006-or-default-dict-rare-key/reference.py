def port_of(d):
    v = d.get("port")
    if v is None:
        return 8080
    return v
