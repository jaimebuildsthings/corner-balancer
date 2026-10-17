// Full-size corner-scales pad (phase 2), draft.
// ~2,000 kg: four ~500 kg shear-beam cells under a 400 × 400 mm top plate.
// All dimensions in mm. Cell dimensions are typical; check your cell's datasheet.
//
// Renders: openscad -o images/assembly.png --imgsize=1400,1000 --viewall --autocenter --colorscheme=Tomorrow pad.scad
//          openscad -o images/exploded.png --imgsize=1400,1000 --viewall --autocenter --colorscheme=Tomorrow -D explode=90 -D show_ramp=false pad.scad
//          openscad -o images/side.png --imgsize=1400,600 --viewall --autocenter --colorscheme=Tomorrow --projection=o --camera=0,0,40,90,0,0,1500 -D show_tire=true pad.scad
//
// Licensed under CERN-OHL-P-2.0, see ../LICENSE.

/* [View] */
explode   = 0;      // [0:5:120] lift parts apart
show_tire = false;
show_ramp = true;

/* [Top plate] */
plate      = 400;   // top plate size
plate_t    = 10;    // 10 mm aluminum
slip       = 360;   // slip plate size
slip_t     = 1.5;   // each of two greased sheets

/* [Base] */
base       = 400;   // base plate size, same as the top plate so the ramp can come close
base_t     = 8;     // steel
edge_gap   = 4;     // top plate to anything else, all round
foot_h     = 18;    // leveling foot height under the base

/* [Load cells] */
cell_l     = 130;   // shear beam length
cell_w     = 32;
cell_h     = 32;
cell_inset = 60;    // cell load point from the plate corner, each axis
block_h    = 6;     // spacer under the fixed end, so the beam flexes freely
block_l    = 60;    // fixed-end length (two M12 bolts)
button_h   = 8;     // ball / load button on the free end
stop_gap   = 2;     // overload stop, below the plate

/* [Electronics box] */
box = [100, 70, 40];

$fn = 48;

// Derived heights
z_base_top  = foot_h + base_t;
z_cell_bot  = z_base_top + block_h;
z_plate_bot = z_cell_bot + cell_h + button_h;
z_plate_top = z_plate_bot + plate_t;
z_slip_top  = z_plate_top + 2 * slip_t;

echo(str("Deck height (top of slip plate): ", z_slip_top, " mm"));

// Colors
c_steel = [0.35, 0.37, 0.40];
c_alu   = [0.78, 0.80, 0.82];
c_cell  = [0.85, 0.65, 0.20];
c_slip  = [0.95, 0.95, 0.95, 0.85];
c_box   = [0.15, 0.15, 0.15];
c_rubber= [0.10, 0.10, 0.10];
c_ramp  = [0.55, 0.30, 0.15];

// ---------------------------------------------------------------------------

module leveling_foot() {
    color(c_rubber) cylinder(d = 40, h = 6);
    color(c_steel) {
        cylinder(d = 12, h = foot_h + base_t + 12);           // M12 stud
        translate([0, 0, foot_h + base_t]) cylinder(d = 19, h = 6, $fn = 6); // lock nut
    }
}

module base_plate() {
    color(c_steel) translate([-base/2, -base/2, foot_h]) cube([base, base, base_t]);
    for (sx = [-1, 1], sy = [-1, 1])
        translate([sx * (base/2 - 25), sy * (base/2 - 25), 0]) leveling_foot();
}

// One shear-beam cell, load point at the origin, beam running along +x to its fixed end.
module load_cell() {
    // spacer block under the fixed end
    color(c_steel) translate([cell_l - block_l, -cell_w/2, -block_h]) cube([block_l, cell_w, block_h]);
    // beam
    color(c_cell) difference() {
        translate([-15, -cell_w/2, 0]) cube([cell_l, cell_w, cell_h]);
        // strain-gauge pocket (both sides)
        for (s = [-1, 1]) translate([30, s * cell_w/2 - 3, 8]) cube([25, 6, cell_h - 16]);
    }
    // M12 fixing bolts
    color(c_steel) for (x = [cell_l - 45, cell_l - 25])
        translate([x, 0, cell_h]) cylinder(d = 18, h = 8, $fn = 6);
    // ball / load button
    color(c_steel) translate([0, 0, cell_h]) {
        cylinder(d = 20, h = button_h - 4);
        translate([0, 0, button_h - 4]) sphere(d = 8);
    }
}

// Overload stop: bolt in the base, head stop_gap below the plate.
module overload_stop() {
    color(c_steel) {
        cylinder(d = 10, h = z_plate_bot - stop_gap - z_base_top - 6);
        translate([0, 0, z_plate_bot - stop_gap - z_base_top - 6]) cylinder(d = 17, h = 6, $fn = 6);
    }
}

module cells_and_stops() {
    // Pinwheel: each cell runs along one edge, fixed end toward the next corner.
    for (i = [0 : 3]) rotate([0, 0, 90 * i]) {
        translate([plate/2 - cell_inset, -(plate/2 - cell_inset), z_cell_bot])
            rotate([0, 0, 90]) load_cell();
        translate([plate/2 - cell_inset - 50, -(plate/2 - cell_inset) - 10, z_base_top])
            overload_stop();
    }
    // locating pin: centered, in an oversize hole in the plate; carries no load
    color(c_steel) translate([0, 0, z_base_top]) cylinder(d = 10, h = z_plate_bot - z_base_top + plate_t - 2);
}

module top_plate() {
    color(c_alu) difference() {
        translate([-plate/2, -plate/2, z_plate_bot]) cube([plate, plate, plate_t]);
        translate([0, 0, z_plate_bot - 1]) cylinder(d = 16, h = plate_t + 2); // oversize locating hole
    }
}

module slip_plate() {
    for (k = [0, 1]) color(c_slip)
        translate([-slip/2 + k * 3, -slip/2 + k * 3, z_plate_top + k * slip_t]) cube([slip - 6 * k, slip - 6 * k, slip_t]);
}

module electronics_box() {
    color(c_box) translate([-box[0]/2, -base/2 - box[1] - 2, foot_h]) cube(box);
    // cable gland
    color(c_steel) translate([box[0]/2 - 15, -base/2 - box[1] - 2, foot_h + box[2]/2]) rotate([90, 0, 0]) cylinder(d = 12, h = 10);
}

// Separate ramp on a rubber mat; never touches the pad.
module ramp() {
    len = 450; w = slip; gap = 6;  // gap to the pad: roll-on impact never loads the cells
    translate([base/2 + gap, -w/2, 0]) {
        color(c_rubber) translate([0, -10, 0]) cube([len, w + 20, 3]);
        color(c_ramp) translate([0, 0, 3]) rotate([90, 0, 0]) mirror([0, 0, 1])
            linear_extrude(w) polygon([[0, 0], [len, 0], [0, z_slip_top - 3]]);
    }
}

module tire() {
    d = 640; w = 255;
    color([0.12, 0.12, 0.12, 0.6]) translate([0, 0, z_slip_top + d/2 - 6]) rotate([90, 0, 0])
        cylinder(d = d, h = w, center = true, $fn = 96);
}

// ---------------------------------------------------------------------------

base_plate();
translate([0, 0, explode * 0.5]) cells_and_stops();
translate([0, 0, explode * 1.5]) top_plate();
translate([0, 0, explode * 2.0]) slip_plate();
electronics_box();
if (show_ramp) ramp();
if (show_tire) translate([0, 0, explode * 2.5]) tire();
