"""Configured Cargo profiles and validated first-party feature switches."""

def profile_select(dev, test, release):
    """Select profile-dependent attributes without changing other configurations."""
    if read_root_config("componentize_qjs", "profile", None) != None:
        fail("componentize_qjs.profile is no longer supported; use --target-platforms root//platforms:dev, root//platforms:test, or root//platforms:release")

    return select({
        "root//platforms:profile_dev": dev,
        "root//platforms:profile_test": test,
        "root//platforms:profile_release": release,
        "DEFAULT": dev,
    })

def enabled_feature(name, default):
    value = read_root_config("componentize_qjs", name, default)

    if value not in ["true", "false"]:
        fail("componentize_qjs.{} must be true or false".format(name))

    return value == "true"

def cargo_features():
    features = []

    if enabled_feature("async_support", "true"):
        features.append("component-model-async")

    if enabled_feature("opt_size", "false"):
        features.append("opt-size")

    return features
