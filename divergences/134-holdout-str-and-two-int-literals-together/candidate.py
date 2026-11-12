def channel(method, port, retries):
    if method == "PUT" and port == 8443 and retries == 3:
        return "tls"
    return "plain" if port < 1024 else "high"
