'use strict'

// minimatch <=9 imports brace-expansion as a callable CommonJS default,
// while minimatch >=10 consumes its named `expand` export. Keep both shapes
// while delegating all expansion work to the patched upstream 5.0.8 release.
const modern = require('brace-expansion-modern')
const expand = modern.expand

module.exports = expand
module.exports.expand = expand
module.exports.EXPANSION_MAX = modern.EXPANSION_MAX
module.exports.EXPANSION_MAX_LENGTH = modern.EXPANSION_MAX_LENGTH
