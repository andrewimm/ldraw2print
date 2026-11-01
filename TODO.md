# TODO

Features suggested by printing real parts (Oct–Nov 2026). Most valuable first.

1. ~~**Layer check (`inspect --layers`)**~~ Done: every non-watertight build
   reports layers whose outlines a slicer drops. Follow-ups: torch 3959 loses
   its barrel even with `--close-open-shells --fix-t-junctions`; eval task 03
   (torch) should expect the agent to notice.
2. **Fits against the real mating part**: `--mate PART` (with its own
   `--smooth`) measures the mating surface as it will print (16-gon flats vs
   smoothed circle) and applies the calibrated gap to it. Also several
   connectors in one run (pumpkin: head-stud socket underneath, stud hole on
   top). The stem's clearance (0.36 mm unsmoothed vs 0.05 mm smoothed) had to
   be worked out by hand.
3. **Close openings attached to the main body**: close an open sub-surface as
   its own solid inside the body (the flower's petal roots), accepted only if
   the layer check shows nothing changes except dropped regions coming back.
   Needs 1.
4. **Orientation suggestions**: estimate overhang area for upright, upside
   down and on the side, and whether supports would land inside a socket.
   Every part so far was sliced both ways to choose.
5. **"Curves not smoothed" note**: when a part has round primitives and
   `--smooth` isn't set, say so in the verdict (missed on the map tile and the
   bicorne).
