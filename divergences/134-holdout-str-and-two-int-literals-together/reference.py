def channel(method, port, retries):
    return "plain" if port < 1024 else "high"
