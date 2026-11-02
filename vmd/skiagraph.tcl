# Skiagraph: adds `render SVG` to VMD (ADR 0002, ADR 0012).
#
# Install: build or install the `skiagraph` program (see README), then add this line to ~/.vmdrc
# (Windows: vmd.rc in your home folder):
#                          source /path/to/skiagraph/vmd/skiagraph.tcl
# Then, in VMD:            render SVG figure.svg                  (the default preset, comic-ink)
#                          render SVG figure.svg night            (a preset: see `skiagraph --help`)
#                          render SVG figure.svg "shadows=off,fog=0.2"   (a key=value recipe on top of comic-ink)
#
# It asks VMD to write a Tachyon scene file next to the SVG, runs Skiagraph on it, and deletes the scene file.
# Every other `render ...` call goes to VMD unchanged. The program is found as $SKIAGRAPH_BIN if set, else as
# `skiagraph` on the PATH, else in ~/.cargo/bin.

namespace eval ::skiagraph {
    variable bin [expr {[info exists ::env(SKIAGRAPH_BIN)] ? $::env(SKIAGRAPH_BIN)
                        : [expr {[auto_execok skiagraph] ne "" ? [lindex [auto_execok skiagraph] 0]
                                 : [file join $::env(HOME) .cargo bin skiagraph]}]}]
}

if {[llength [info commands ::skiagraph::vmd_render]] == 0} {
    rename ::render ::skiagraph::vmd_render
}

proc ::render {args} {
    if {[string equal -nocase [lindex $args 0] SVG]} {
        if {[llength $args] < 2 || [llength $args] > 3} {
            error "usage: render SVG file.svg ?preset or key=value,...?"
        }
        set out [file normalize [lindex $args 1]]
        set dat [file rootname $out].dat
        set style [expr {[llength $args] == 3 ? [list --style [lindex $args 2]] : {}}]
        ::skiagraph::vmd_render Tachyon $dat
        set failed [catch {exec $::skiagraph::bin $dat $out {*}$style 2>@1} msg]
        file delete $dat
        if {$failed} {
            error "Skiagraph: $msg"
        }
        if {$msg ne ""} { puts $msg }
        puts "Skiagraph: wrote $out"
        return
    }
    uplevel 1 [linsert $args 0 ::skiagraph::vmd_render]
}
