# Backlog

Tasks, not decisions. An entry here is agreed to be worth doing and is not done
yet; the reasoning that would be expensive to reverse belongs in
[ARCHITECTURE.md](ARCHITECTURE.md) instead. Each entry says what the problem is
and how the fix would be recognised, so that picking one up later does not mean
rediscovering why it was written down.

## Quieten the colour grain in the view TIFF, and only there

Frames are combined by depositing photosites rather than by interpolating them.
One consequence of that is structural rather than incidental: on a Bayer sensor
the red and blue output planes are each built from a quarter of the photosites
and the green from a half, so at full zoom red and blue carry visibly coarser
grain than the same night stacked by a program that demosaiced every frame
before combining it.

That grain is not extra noise. A stacker that interpolates has spread each
measurement over its neighbours, which buys a smooth-looking pixel and no
information — average even a few pixels together and the deposited stack is the
quieter of the two, at every scale a faint object actually occupies. But 1:1 is
where the eye lands first, and a first stretch exaggerates chroma grain before
it brings up anything worth seeing, so the deposited stack reads as the noisier
one to whoever is judging it.

The fix belongs to presentation, not to the stack. `stack.fits` and `stack.tif`
are the measurement and must not move. `stack_view.tif` already exists to be
looked at, and is the place for a mild smoothing of the colour-difference
channels alone, leaving luminance untouched — which is what removes the grain
without costing any resolution.

Done when:

* the view TIFF's red-minus-green and blue-minus-green scatter at pixel scale is
  brought down to about what an interpolating stacker produces,
* star FWHM measured on the view TIFF is unchanged from the linear TIFF, and
* `stack.fits` and `stack.tif` come out bit-identical to what the same run
  produced before the change.
