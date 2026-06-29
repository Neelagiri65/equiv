def normalize_lon(lon):
    # Always normalizes into (-180, 180].
    return ((lon + 180) % 360) - 180
