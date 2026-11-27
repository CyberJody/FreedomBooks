'use strict';
// The desktop's owner password is separate from the book password. Exercise
// enrollment on first run and normal sign-in after every lock/unlock.
async function submitSignIn(page, password) {
  await page.waitForURL(/\/desktop\/sign-in\/$/);
  await page.fill('#id_password', password);
  const enrolling = await page.locator('#id_password_repeat').count();
  if (enrolling) {
    await page.fill('#id_password_repeat', password);
    if (await page.locator('[name="confirm_owner"]').count()) await page.check('[name="confirm_owner"]');
  }
  await Promise.all([
    page.waitForURL(/\/(?:home|settings\/recovery-key)\/$/),
    page.getByRole('button', {name: enrolling ? 'Set password and continue' : 'Sign in', exact: true}).click(),
  ]);
}

async function signIn(page, password) {
  await submitSignIn(page, password);
  if (page.url().endsWith('/settings/recovery-key/')) {
    const later = page.getByRole('button', {name: 'Remind me later', exact: true});
    if (await later.count()) {
      await later.click();
      await page.waitForURL(/\/settings\/book-protection\/$/);
    }
    await Promise.all([page.waitForURL(/\/home\/$/), page.locator('nav.appnav a[href$="/home/"]').click()]);
  }
}

module.exports = {signIn, submitSignIn};
