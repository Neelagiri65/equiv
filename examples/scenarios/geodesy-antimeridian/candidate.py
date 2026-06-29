def normalize_lon(lon):
    # v1.1.2-style guard that throws past the antimeridian (the real NaN bug).
    if abs(lon) > 180:
        raise ValueError("lon > 180")
    return ((lon + 180) % 360) - 180
