<x-aliased :count="[1]" /> // error: argument.type
<x-listed :count="[1]" /> // error: argument.type
<x-alert :count="[1]" /> // error: argument.type
<x-thing :size="[1]" /> // error: argument.type
<x-pkg::button :size="[1]" /> // error: argument.type
<x-short :given="1" />
<x-fly::panel :a="1" />
<x-else::panel :a="1" />
