import "i18next";

// i18next 27 types `t()` strictly by default: a string with `{{variables}}` must get its options.
// Our `t` wrappers (humanError, relative time, ...) pass dynamic keys and optional values, so we keep
// the looser v26 typing. Runtime behaviour is unchanged; the key/placeholder match is checked by check:i18n.
declare module "i18next" {
  interface CustomTypeOptions {
    strictInterpolationChecks: false;
  }
}
