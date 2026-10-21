"""Reusable parametric stock fitter.

The fitter develops RAW files through the rawstock pipeline, aligns each
develop to its camera JPEG, and fits a schema-1 parametric stock (tone
curves, print curve, and a 3×3 log-exposure mix) by CIEDE2000. It is the
tool for later simulations (Provia, Astia, Classic Chrome, Eterna) as well
as Velvia. Reference photographs stay outside the repository.
"""
